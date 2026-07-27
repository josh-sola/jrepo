use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

use crate::git;
use crate::store::{self, Repo, Tree, TreeState};

const FETCH_STALE_AFTER: chrono::Duration = chrono::Duration::minutes(5);

pub struct NewOptions {
    pub repo: String,
    pub name: String,
    pub branch: Option<String>,
    pub profiles: Option<Vec<String>>,
}

pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_hyphen = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen && !slug.is_empty() {
            slug.push('-');
            last_was_hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

pub fn new_tree(root: &Path, opts: NewOptions) -> Result<PathBuf> {
    let store = store::load(root)?;
    let repo = store.repos.get(&opts.repo).cloned().with_context(|| {
        format!(
            "unknown repo '{}'. Known repos: {}",
            opts.repo,
            known_repos(&store)
        )
    })?;

    let needs_fetch = match repo.last_fetch {
        None => true,
        Some(t) => Utc::now() - t > FETCH_STALE_AFTER,
    };
    if needs_fetch {
        eprintln!("fetching {}...", opts.repo);
        git::fetch_prune(&repo.base)?;
        store::with_store_lock(root, |s| {
            if let Some(r) = s.repos.get_mut(&opts.repo) {
                r.last_fetch = Some(Utc::now());
            }
            Ok(())
        })?;
    }

    let branch = match opts.branch.clone() {
        Some(b) => b,
        None => {
            let slug = slugify(&opts.name);
            if slug.is_empty() {
                bail!(
                    "'{}' has no alphanumeric characters to build a branch name from; pass --branch explicitly",
                    opts.name
                );
            }
            format!("{}{}", repo.branch_prefix, slug)
        }
    };
    if git::branch_exists_local(&repo.base, &branch)? {
        bail!("branch '{branch}' already exists locally");
    }
    if git::branch_exists_remote(&repo.base, &branch)? {
        bail!("branch '{branch}' already exists on origin");
    }

    let id = Uuid::now_v7();
    let repo_dir = root.join(&opts.repo);
    let tree_path = repo_dir.join("trees").join(id.to_string());
    let start_point = format!("origin/{}", repo.trunk);
    git::worktree_add(&repo.base, &tree_path, &branch, &start_point)?;
    let tree_path = fs::canonicalize(&tree_path)?;
    let log_path = tree_path.join(crate::repo::PROVISION_LOG_NAME);

    // Registered while still `Provisioning` so a failure in wiring or a
    // step below lands as a `Failed` entry, not an orphan invisible to
    // `wt ls`/`wt rm`.
    let now = Utc::now();
    store::with_store_lock(root, |s| {
        s.trees.push(Tree {
            id,
            repo: opts.repo.clone(),
            name: opts.name.clone(),
            branch: branch.clone(),
            path: tree_path.clone(),
            created: now,
            state: TreeState::Provisioning,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: Some(log_path.clone()),
        });
        Ok(())
    })?;

    if let Err(e) = wire_shared_symlinks(&repo_dir.join("shared"), &tree_path, &repo.shared)
        .and_then(|()| copy_globs(&repo.base, &tree_path, &repo.copy, &repo.shared))
    {
        return mark_failed(
            root,
            id,
            &tree_path,
            &format!("wiring shared state failed:\n{e:#}\n"),
            "wiring shared state failed",
        );
    }

    spawn_background_provisioning(root, id, &log_path, &opts.profiles)?;

    println!("{}", tree_path.display());
    Ok(tree_path)
}

/// Re-execs the binary as `wt __provision` instead of threading, so the
/// parent can exit and the OS reparents the child rather than a live handle
/// (and thread) pinning it to a process that is about to go away.
/// `process_group(0)` detaches it from the parent's process group so a
/// Ctrl-C at the terminal that spawned `wt new` doesn't also signal it.
fn spawn_background_provisioning(
    root: &Path,
    id: Uuid,
    log_path: &Path,
    profiles: &Option<Vec<String>>,
) -> Result<()> {
    let exe = std::env::current_exe().context("resolving current executable")?;
    let stdout_log =
        fs::File::create(log_path).with_context(|| format!("creating {}", log_path.display()))?;
    let stderr_log = stdout_log
        .try_clone()
        .with_context(|| format!("cloning handle for {}", log_path.display()))?;

    let mut cmd = Command::new(exe);
    cmd.arg("__provision").arg(id.to_string());
    if let Some(profiles) = profiles {
        cmd.arg("--profile").arg(profiles.join(","));
    }
    cmd.env("WT_ROOT", root)
        .stdin(Stdio::null())
        .stdout(stdout_log)
        .stderr(stderr_log)
        .process_group(0);

    cmd.spawn().context("spawning background provisioning")?;
    Ok(())
}

/// Leaves the tree on disk and registered as `Failed` rather than cleaning
/// up — a half-provisioned tree is still worth inspecting or resuming by
/// hand, and deleting it would throw away whatever steps did complete.
fn mark_failed(
    root: &Path,
    id: Uuid,
    tree_path: &Path,
    log_contents: &str,
    message: &str,
) -> Result<PathBuf> {
    let log_path = tree_path.join(crate::repo::PROVISION_LOG_NAME);
    fs::write(&log_path, log_contents)
        .with_context(|| format!("writing {}", log_path.display()))?;
    store::with_store_lock(root, |s| {
        if let Some(t) = s.trees.iter_mut().find(|t| t.id == id) {
            t.state = TreeState::Failed;
        }
        Ok(())
    })?;
    eprintln!("{message}; see {}", log_path.display());
    println!("{}", tree_path.display());
    bail!("{message}");
}

fn known_repos(store: &store::Store) -> String {
    if store.repos.is_empty() {
        "(none registered)".to_string()
    } else {
        store.repos.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// A real file or directory at a shared path means it is tracked in git —
/// clobbering it would destroy content the repo owns, so this only warns.
fn wire_shared_symlinks(shared_root: &Path, tree_path: &Path, shared: &[String]) -> Result<()> {
    for relpath in shared {
        let dst = tree_path.join(relpath);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::symlink_metadata(&dst) {
            Ok(_) => {
                eprintln!(
                    "warning: {} already exists in the tree; leaving it in place instead of symlinking to shared state",
                    dst.display()
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let target = shared_root.join(relpath);
                symlink(&target, &dst).with_context(|| format!("symlinking {}", dst.display()))?;
            }
            Err(e) => return Err(e).with_context(|| format!("checking {}", dst.display())),
        }
    }
    Ok(())
}

/// Supports the one glob subset actually needed: an optional leading
/// `**/` (recurse everywhere) plus a single `*` wildcard in the filename.
fn matches_glob(pattern: &str, filename: &str) -> bool {
    let pattern = pattern.strip_prefix("**/").unwrap_or(pattern);
    match pattern.split_once('*') {
        Some((head, tail)) => {
            filename.starts_with(head)
                && filename.ends_with(tail)
                && filename.len() >= head.len() + tail.len()
        }
        None => filename == pattern,
    }
}

/// Matches patterns against git's ignored-file list instead of walking the
/// filesystem: a plain walk would stat every file in whatever the repo
/// gitignores wholesale (build caches, `node_modules`, `.venv`s) just to
/// find a dozen `.env` files. A tracked file is never a candidate either —
/// if it's tracked it's already in the worktree.
fn copy_globs(base: &Path, tree_path: &Path, patterns: &[String], shared: &[String]) -> Result<()> {
    if patterns.is_empty() {
        return Ok(());
    }
    let shared_paths: Vec<&Path> = shared.iter().map(Path::new).collect();

    for relpath in git::ignored_files(base)? {
        let rel = Path::new(&relpath);
        if shared_paths.iter().any(|s| rel.starts_with(s)) {
            continue;
        }
        let Some(file_name) = rel.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        if !patterns.iter().any(|p| matches_glob(p, file_name)) {
            continue;
        }

        let src = base.join(rel);
        let dst = tree_path.join(rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&src, &dst)
            .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
    }
    Ok(())
}

pub fn rm_tree(root: &Path, selector: &str, force: bool, delete_branch: bool) -> Result<()> {
    let store = store::load(root)?;
    let tree = store::resolve(&store.trees, selector)?;
    let id = tree.id;
    let name = tree.name.clone();
    let branch = tree.branch.clone();
    let tree_path = tree.path.clone();
    let repo = store
        .repos
        .get(&tree.repo)
        .with_context(|| format!("tree '{name}' references unknown repo '{}'", tree.repo))?
        .clone();

    let unpushed = branch_has_unpushed_commits(&repo.base, &branch, &repo.trunk)?;

    // A path that's already gone is drift, not a removal to perform: there
    // is nothing left to protect by refusing, so it skips the dirty/unpushed
    // guard entirely and goes straight to unregistering below.
    if tree_path.exists() {
        if !force {
            if git::is_dirty(&tree_path)? {
                bail!("tree '{name}' has uncommitted changes; use --force to remove anyway");
            }
            if unpushed {
                bail!("tree '{name}' has commits not on the remote; use --force to remove anyway");
            }
        }

        let remove_result = remove_worktree(&repo.base, &tree_path, force);
        if tree_path.exists() {
            let err = match remove_result {
                Ok(()) => anyhow::anyhow!(
                    "git reported success but {} is still on disk",
                    tree_path.display()
                ),
                Err(e) => e,
            };
            bail!(
                "failed to remove worktree at {}: {err:#}. The registry entry is kept; remove it \
                 by hand (git -C {} worktree remove --force \"{}\") or run `wt doctor --fix`.",
                tree_path.display(),
                repo.base.display(),
                tree_path.display()
            );
        }
        if let Err(e) = remove_result {
            eprintln!("warning: {e:#} (the worktree directory is already gone; treating as drift)");
        }
    } else {
        eprintln!(
            "{} no longer exists on disk; unregistering '{name}' as drift",
            tree_path.display()
        );
    }

    if let Err(e) = git::worktree_prune(&repo.base) {
        eprintln!("warning: git worktree prune failed: {e}");
    }

    store::with_store_lock(root, |s| {
        s.trees.retain(|t| t.id != id);
        Ok(())
    })?;

    if delete_branch {
        if unpushed {
            eprintln!("keeping branch '{branch}': it has commits not on the remote");
        } else if let Err(e) = git::delete_branch(&repo.base, &branch) {
            eprintln!("warning: could not delete branch '{branch}': {e}");
        }
    }

    Ok(())
}

/// A submodule-containing tree makes `git worktree remove` refuse outright;
/// deinit first and fall back to attempting removal anyway if that fails,
/// so an unrelated submodule problem never blocks a removal that would
/// otherwise succeed. Verified empirically: even after a clean deinit, git
/// still refuses a former submodule path without `--force` on the remove
/// itself, so a tree with submodules always gets `--force` regardless of
/// the caller's own flag — our dirty/unpushed checks already ran earlier.
fn remove_worktree(base: &Path, tree_path: &Path, force: bool) -> Result<()> {
    let mut force = force;
    if tree_path.join(".gitmodules").exists() {
        if let Err(e) = git::submodule_deinit_all(tree_path) {
            eprintln!("warning: submodule deinit failed ({e}); attempting removal anyway");
        }
        force = true;
    }
    git::worktree_remove(base, tree_path, force)
}

fn branch_has_unpushed_commits(base: &Path, branch: &str, trunk: &str) -> Result<bool> {
    match git::branch_upstream(base, branch) {
        Some(upstream) => git::commits_ahead(base, &format!("{upstream}..{branch}")),
        None => git::commits_ahead(base, &format!("origin/{trunk}..{branch}")),
    }
}

pub struct GcOptions {
    pub repo: Option<String>,
    pub dry_run: bool,
}

pub fn gc(root: &Path, opts: GcOptions) -> Result<()> {
    let store = store::load(root)?;
    let mut candidates = 0;

    for t in &store.trees {
        if let Some(ref r) = opts.repo
            && &t.repo != r
        {
            continue;
        }
        let Some(repo) = store.repos.get(&t.repo) else {
            eprintln!("skipping '{}': repo '{}' is not registered", t.name, t.repo);
            continue;
        };
        match gc_skip_reason(repo, t) {
            Ok(Some(reason)) => {
                eprintln!("skipping '{}': {reason}", t.name);
                continue;
            }
            Err(e) => {
                eprintln!("skipping '{}': {e:#}", t.name);
                continue;
            }
            Ok(None) => {}
        }

        candidates += 1;
        if opts.dry_run {
            println!("would reap '{}' ({})", t.name, t.path.display());
            continue;
        }
        println!("reaping '{}' ({})", t.name, t.path.display());
        if let Err(e) = rm_tree(root, &t.id.to_string(), false, true) {
            eprintln!("failed to reap '{}': {e:#}", t.name);
        }
    }

    if candidates == 0 {
        println!("nothing to reap");
    }
    Ok(())
}

/// The trunk-relative check, not the upstream-relative one `rm` uses: a
/// branch pushed to its own remote counterpart has no *unpushed* commits
/// but can still carry real work ahead of trunk, which gc must leave alone.
fn gc_skip_reason(repo: &Repo, tree: &Tree) -> Result<Option<String>> {
    if tree.state == TreeState::Provisioning {
        return Ok(Some("still provisioning".to_string()));
    }
    // A failed tree is clean and sits at trunk, so every check below would
    // wave it through and take its provisioning log with it.
    if tree.state == TreeState::Failed {
        return Ok(Some(
            "provisioning failed; read its log, then remove it with `wt rm`".to_string(),
        ));
    }
    if git::is_dirty(&tree.path)? {
        return Ok(Some("uncommitted changes".to_string()));
    }
    if git::commits_ahead(
        &repo.base,
        &format!("origin/{}..{}", repo.trunk, tree.branch),
    )? {
        return Ok(Some(format!("commits ahead of origin/{}", repo.trunk)));
    }
    Ok(None)
}

pub struct DoctorOptions {
    pub fix: bool,
}

pub fn doctor(root: &Path, opts: DoctorOptions) -> Result<()> {
    let store = store::load(root)?;

    for (repo_name, repo) in &store.repos {
        println!("== {repo_name} ==");
        let worktrees = git::worktree_list(&repo.base)?;
        let registered: Vec<&Tree> = store
            .trees
            .iter()
            .filter(|t| &t.repo == repo_name)
            .collect();
        let mut stale_ids = Vec::new();

        for t in &registered {
            if !t.path.exists() {
                println!(
                    "  stale registry entry: '{}' — {} no longer exists",
                    t.name,
                    t.path.display()
                );
                stale_ids.push(t.id);
            }
        }

        for w in &worktrees {
            if w.path == repo.base {
                continue;
            }
            if !registered.iter().any(|t| t.path == w.path) {
                let branch = w.branch.as_deref().unwrap_or("(detached)");
                println!(
                    "  unregistered worktree: {} (branch {branch}) — not tracked by wt; leave it \
                     alone or register it by hand if you want wt to manage it",
                    w.path.display()
                );
            }
        }

        for t in &registered {
            if !t.path.exists() {
                continue;
            }
            match worktrees.iter().find(|w| w.path == t.path) {
                Some(w) if w.branch.as_deref() != Some(t.branch.as_str()) => {
                    let actual = w.branch.as_deref().unwrap_or("(detached)");
                    println!(
                        "  branch mismatch: '{}' registered as '{}' but checked out as {actual}",
                        t.name, t.branch
                    );
                }
                Some(_) => {}
                None => println!(
                    "  drifted: '{}' exists on disk at {} but git no longer lists it as a worktree",
                    t.name,
                    t.path.display()
                ),
            }
        }

        if opts.fix {
            if !stale_ids.is_empty() {
                let n = stale_ids.len();
                store::with_store_lock(root, |s| {
                    s.trees.retain(|t| !stale_ids.contains(&t.id));
                    Ok(())
                })?;
                println!(
                    "  removed {n} stale registry entr{}",
                    if n == 1 { "y" } else { "ies" }
                );
            }
            git::worktree_prune(&repo.base)?;
            println!("  pruned {repo_name}'s worktree list");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

    #[test]
    fn gc_skips_a_failed_tree() {
        let repo = Repo {
            base: PathBuf::from("/nonexistent-base"),
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            last_fetch: None,
            shared: Vec::new(),
            copy: Vec::new(),
            env: Default::default(),
            steps: Vec::new(),
        };
        let tree = Tree {
            id: Uuid::now_v7(),
            repo: "myrepo".into(),
            name: "half provisioned".into(),
            branch: "josh/half-provisioned".into(),
            path: PathBuf::from("/nonexistent-tree"),
            created: Utc::now(),
            state: TreeState::Failed,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
        };

        let reason = gc_skip_reason(&repo, &tree).unwrap().unwrap();
        assert!(
            reason.contains("provisioning failed"),
            "unexpected reason: {reason}"
        );
    }

    #[test]
    fn slugify_lowercases_and_collapses_non_alnum() {
        assert_eq!(slugify("wt cli bootstrap"), "wt-cli-bootstrap");
        assert_eq!(slugify("Fix: the thing!!"), "fix-the-thing");
        assert_eq!(slugify("  leading and trailing  "), "leading-and-trailing");
        assert_eq!(slugify("a---b"), "a-b");
    }

    #[test]
    fn slugify_of_symbols_only_is_empty() {
        // `new_tree` treats this as the signal to require an explicit
        // `--branch` instead of building `<prefix>` with nothing after it.
        assert_eq!(slugify("???"), "");
        assert_eq!(slugify("!!!  ---"), "");
    }

    #[test]
    fn glob_matches_env_files_recursively() {
        assert!(matches_glob("**/.env*", ".env"));
        assert!(matches_glob("**/.env*", ".env.local"));
        assert!(!matches_glob("**/.env*", "env.ts"));
        assert!(matches_glob("README.md", "README.md"));
        assert!(!matches_glob("README.md", "readme.md"));
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wt-tree-test-{label}-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn init_git_repo(dir: &Path) {
        let status = Command::new("git")
            .args(["init", "-q", "."])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn copy_globs_queries_git_instead_of_walking_ignored_directories() {
        let base = temp_dir("copy-base");
        init_git_repo(&base);
        fs::write(base.join(".env"), "SECRET=1").unwrap();

        // A directory large enough that a full filesystem walk would be
        // the dominant cost if `copy_globs` ever regressed to one.
        let big = base.join("big_ignored");
        fs::create_dir_all(&big).unwrap();
        for i in 0..2000 {
            fs::write(big.join(format!("f{i}.txt")), "x").unwrap();
        }
        fs::write(base.join(".gitignore"), ".env\nbig_ignored/\n").unwrap();

        let tree_path = temp_dir("copy-dst");
        let start = Instant::now();
        copy_globs(&base, &tree_path, &["**/.env*".to_string()], &[]).unwrap();
        let elapsed = start.elapsed();

        assert_eq!(
            fs::read_to_string(tree_path.join(".env")).unwrap(),
            "SECRET=1"
        );
        assert!(!tree_path.join("big_ignored").exists());
        assert!(
            elapsed.as_secs() < 5,
            "copy_globs took {elapsed:?}, expected a git query, not a walk"
        );
    }

    #[test]
    fn copy_globs_skips_shared_relpaths() {
        let base = temp_dir("copy-shared-base");
        init_git_repo(&base);
        fs::create_dir_all(base.join("plans")).unwrap();
        fs::write(base.join("plans").join(".env"), "SHARED=1").unwrap();
        fs::write(base.join(".env"), "ROOT=1").unwrap();
        fs::write(base.join(".gitignore"), ".env\nplans/.env\n").unwrap();

        let tree_path = temp_dir("copy-shared-dst");
        copy_globs(
            &base,
            &tree_path,
            &["**/.env*".to_string()],
            &["plans".to_string()],
        )
        .unwrap();

        assert!(tree_path.join(".env").exists());
        assert!(!tree_path.join("plans").exists());
    }
}
