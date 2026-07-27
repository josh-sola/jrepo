use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

use crate::git;
use crate::store::{self, Tree, TreeState};

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

    let steps_to_run: Vec<_> = repo
        .steps
        .iter()
        .filter(|s| match &opts.profiles {
            None => true,
            Some(profiles) => profiles.iter().any(|p| p == &s.profile),
        })
        .collect();

    for step in &steps_to_run {
        eprintln!("provisioning: {}", step.label);
        let cwd = tree_path.join(&step.cwd);
        let output = Command::new(&step.cmd[0])
            .args(&step.cmd[1..])
            .current_dir(&cwd)
            .envs(&repo.env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("running step '{}'", step.label))?;

        if !output.status.success() {
            let mut log = format!(
                "step: {}\ncmd: {:?}\n\n--- stdout ---\n",
                step.label, step.cmd
            );
            log.push_str(&String::from_utf8_lossy(&output.stdout));
            log.push_str("\n--- stderr ---\n");
            log.push_str(&String::from_utf8_lossy(&output.stderr));
            return mark_failed(
                root,
                id,
                &tree_path,
                &log,
                &format!("step '{}' failed", step.label),
            );
        }
    }

    store::with_store_lock(root, |s| {
        if let Some(t) = s.trees.iter_mut().find(|t| t.id == id) {
            t.state = TreeState::Ready;
        }
        Ok(())
    })?;

    println!("{}", tree_path.display());
    Ok(tree_path)
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
    let log_path = tree_path.join(".wt-provision.log");
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

pub fn rm_tree(root: &Path, selector: &str, force: bool) -> Result<()> {
    let store = store::load(root)?;
    let tree = store::resolve(&store.trees, selector)?;
    let id = tree.id;
    let tree_path = tree.path.clone();
    let repo = store
        .repos
        .get(&tree.repo)
        .with_context(|| {
            format!(
                "tree '{}' references unknown repo '{}'",
                tree.name, tree.repo
            )
        })?
        .clone();

    if !force {
        if git::is_dirty(&tree_path)? {
            bail!(
                "tree '{}' has uncommitted changes; use --force to remove anyway",
                tree.name
            );
        }
        let unpushed = match git::upstream_ref(&tree_path) {
            Some(upstream) => git::commits_ahead(&tree_path, &format!("{upstream}..HEAD"))?,
            None => git::commits_ahead(&tree_path, &format!("origin/{}..HEAD", repo.trunk))?,
        };
        if unpushed {
            bail!(
                "tree '{}' has commits not on the remote; use --force to remove anyway",
                tree.name
            );
        }
    }

    if let Err(e) = git::worktree_remove(&repo.base, &tree_path, force) {
        eprintln!("warning: {e}; removing registry entry anyway");
    }
    if let Err(e) = git::worktree_prune(&repo.base) {
        eprintln!("warning: git worktree prune failed: {e}");
    }

    store::with_store_lock(root, |s| {
        s.trees.retain(|t| t.id != id);
        Ok(())
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

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
