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
    /// Branch the new tree from here instead of `origin/<trunk>`, joining
    /// whatever Graphite stack this ref belongs to. Resolved by
    /// `resolve_onto`.
    pub onto: Option<String>,
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
    let (id, tree_path, log_path) = create_tree(root, &opts)?;
    start_provisioning(root, id, &log_path, &opts.profiles)?;
    println!("{}", tree_path.display());
    Ok(tree_path)
}

/// Worktree creation, shared-state wiring, and the registry write — stops
/// short of starting provisioning so `wt adopt` can pop its stash into the
/// tree first, before a background install could touch any of the same
/// files. `wt new` runs this immediately followed by `start_provisioning`.
fn create_tree(root: &Path, opts: &NewOptions) -> Result<(Uuid, PathBuf, PathBuf)> {
    create_tree_with(root, opts, "gt")
}

fn create_tree_with(
    root: &Path,
    opts: &NewOptions,
    gt_bin: &str,
) -> Result<(Uuid, PathBuf, PathBuf)> {
    let store = store::load(root)?;
    let repo = store.repos.get(&opts.repo).cloned().with_context(|| {
        format!(
            "unknown repo '{}'. Known repos: {}",
            opts.repo,
            known_repos(&store)
        )
    })?;

    let parent_branch = opts
        .onto
        .as_deref()
        .map(|sel| resolve_onto(&store, &opts.repo, &repo.base, sel))
        .transpose()?;

    // A branch resolved by `--onto` already exists locally; only the
    // trunk-based path needs a fresh `origin/<trunk>` to branch from.
    if parent_branch.is_none() {
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
    let start_point = parent_branch
        .clone()
        .unwrap_or_else(|| format!("origin/{}", repo.trunk));
    git::worktree_add(&repo.base, &tree_path, &branch, &start_point)?;
    if let Err(e) = git::clear_worktree_hooks_path(&tree_path) {
        eprintln!("warning: could not clear inherited worktree hooksPath: {e:#}");
    }
    let tree_path = fs::canonicalize(&tree_path)?;
    if let Some(parent) = &parent_branch {
        track_with_graphite(&tree_path, parent, gt_bin);
    }
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
            provision_pid: None,
            parent_branch: parent_branch.clone(),
        });
        Ok(())
    })?;

    if let Err(e) = wire_shared_symlinks(&repo_dir.join("shared"), &tree_path, &repo.shared)
        .and_then(|()| copy_globs(&repo.base, &tree_path, &repo.copy, &repo.shared))
    {
        return Err(mark_failed::<()>(
            root,
            id,
            &tree_path,
            &format!("wiring shared state failed:\n{e:#}\n"),
            "wiring shared state failed",
        )
        .unwrap_err());
    }

    Ok((id, tree_path, log_path))
}

/// Resolves `--onto`'s selector into the branch `wt new` should create its
/// worktree from, checked in the same tiered, ambiguity-errors order as
/// `store::resolve_index`: a `wt` tree in this repo — by the branch it
/// actually has checked out right now, never `Tree.branch`, which only
/// records what a tree started on — then a local branch name, then any
/// other commit-ish. Ambiguity inside a tier is an error, not a fallthrough
/// to the next tier.
fn resolve_onto(store: &store::Store, repo_name: &str, base: &Path, sel: &str) -> Result<String> {
    let trees: Vec<&Tree> = store.trees.iter().filter(|t| t.repo == repo_name).collect();
    if let Some(branch) = resolve_onto_tree(&trees, sel)? {
        return Ok(branch);
    }
    if git::branch_exists_local(base, sel)? {
        return Ok(sel.to_string());
    }
    if git::rev_parse(base, sel).is_ok() {
        return Ok(sel.to_string());
    }
    bail!("--onto '{sel}' matches no tree, branch, or commit in '{repo_name}'");
}

fn resolve_onto_tree(trees: &[&Tree], sel: &str) -> Result<Option<String>> {
    let needle = sel.to_lowercase();
    let tiers: [fn(&&Tree, &str, &str) -> bool; 4] = [
        |t, s, _| t.id.to_string() == s,
        |t, _, needle| t.id.to_string().starts_with(needle),
        |t, s, _| t.name == s,
        |t, _, needle| t.name.to_lowercase().contains(needle),
    ];
    for tier in tiers {
        let matches: Vec<&&Tree> = trees.iter().filter(|t| tier(t, sel, &needle)).collect();
        match matches.len() {
            0 => continue,
            1 => {
                let t = matches[0];
                let branch = git::current_branch(&t.path).with_context(|| {
                    format!(
                        "reading the branch checked out in '{}' ({})",
                        t.name,
                        t.path.display()
                    )
                })?;
                return Ok(Some(branch));
            }
            _ => {
                let candidates = matches
                    .iter()
                    .map(|t| format!("{} ({})", t.name, t.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("--onto '{sel}' is ambiguous: {candidates}");
            }
        }
    }
    Ok(None)
}

/// Tracks the new tree's branch with Graphite, recording `parent` as its
/// parent. A failure here only warns: the tree already exists and is fully
/// usable, just outside Graphite's stack until `gt track` is run by hand —
/// far better than discarding a freshly created tree over a `gt` hiccup.
fn track_with_graphite(tree_path: &Path, parent: &str, gt_bin: &str) {
    let result = Command::new(gt_bin)
        .args(["track", "--parent", parent, "--no-interactive"])
        .current_dir(tree_path)
        .output();
    match result {
        Ok(out) if out.status.success() => {}
        Ok(out) => eprintln!(
            "warning: `gt track --parent {parent}` failed; the tree exists but Graphite doesn't \
             know its parent yet — fix it by hand with `gt track --parent {parent}` in {}:\n{}",
            tree_path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => eprintln!(
            "warning: could not run `gt track --parent {parent}` in {}: {e:#}",
            tree_path.display()
        ),
    }
}

fn start_provisioning(
    root: &Path,
    id: Uuid,
    log_path: &Path,
    profiles: &Option<Vec<String>>,
) -> Result<()> {
    let pid = spawn_background_provisioning(root, id, log_path, profiles)?;
    store::with_store_lock(root, |s| {
        if let Some(t) = s.trees.iter_mut().find(|t| t.id == id) {
            t.provision_pid = Some(pid);
        }
        Ok(())
    })?;
    Ok(())
}

pub struct AdoptOptions {
    pub repo: Option<String>,
    pub name: String,
    pub branch: Option<String>,
    pub profiles: Option<Vec<String>>,
}

/// Moves uncommitted work out of base into a fresh tree — the escape hatch
/// for when editing started in base by mistake (base blocks commits, not
/// edits). `refs/stash` lives in the common git dir, so it is visible from
/// every worktree of the same clone; that's what lets a stash taken in base
/// be popped straight into the tree `wt new` just created from it, no patch
/// file needed.
///
/// No `--onto` here: the stash was taken against whatever base's `HEAD`
/// already was, so replaying it onto a different branch's tip is a rebase
/// this function doesn't do, and popping it there would surface as merge
/// conflicts with no indication that the mismatch is the real cause.
pub fn adopt(root: &Path, opts: AdoptOptions) -> Result<PathBuf> {
    let store = store::load(root)?;
    let (repo_name, repo) = resolve_adopt_repo(&store, opts.repo)?;

    if !git::is_dirty(&repo.base)? {
        bail!(
            "{repo_name}'s base ({}) is clean; there is nothing to adopt",
            repo.base.display()
        );
    }

    let stash_sha =
        git::stash_push_include_untracked(&repo.base, &format!("wt adopt: {}", opts.name))?;

    let new_opts = NewOptions {
        repo: repo_name.clone(),
        name: opts.name.clone(),
        branch: opts.branch.clone(),
        onto: None,
        profiles: opts.profiles.clone(),
    };
    let (id, tree_path, log_path) = create_tree(root, &new_opts).map_err(|e| {
        anyhow::anyhow!(
            "adopted work is stashed in {repo_name}'s base; creating the tree failed: {e:#}\n\
             recover it with: git -C {} stash pop",
            repo.base.display()
        )
    })?;

    if let Err(e) = git::stash_pop(&tree_path, &stash_sha) {
        return Err(mark_failed::<()>(
            root,
            id,
            &tree_path,
            &format!(
                "git stash pop failed:\n{e:#}\n\nthe stash is intact; resolve the conflict in \
                 the tree and finish by hand with:\n  git -C {} stash pop\nonce resolved, drop \
                 it with:\n  git -C {} stash drop\n",
                tree_path.display(),
                repo.base.display(),
            ),
            "adopt: stash pop failed; the stash is intact",
        )
        .unwrap_err());
    }

    start_provisioning(root, id, &log_path, &opts.profiles)?;
    println!("{}", tree_path.display());
    Ok(tree_path)
}

/// `Some(name)` looks the repo up by name; `None` resolves from the current
/// directory, since `wt adopt` is meant to be run from inside the base
/// checkout it's rescuing work out of.
fn resolve_adopt_repo(store: &store::Store, repo: Option<String>) -> Result<(String, Repo)> {
    if let Some(name) = repo {
        let repo = store.repos.get(&name).cloned().with_context(|| {
            format!("unknown repo '{name}'. Known repos: {}", known_repos(store))
        })?;
        return Ok((name, repo));
    }

    let cwd = std::env::current_dir().context("reading current directory")?;
    let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
    store
        .repos
        .iter()
        .find(|(_, r)| cwd.starts_with(&r.base))
        .map(|(name, repo)| (name.clone(), repo.clone()))
        .context("current directory is not a registered repo's base; pass a repo name")
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
) -> Result<u32> {
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

    let child = cmd.spawn().context("spawning background provisioning")?;
    Ok(child.id())
}

/// Leaves the tree on disk and registered as `Failed` rather than cleaning
/// up — a half-provisioned tree is still worth inspecting or resuming by
/// hand, and deleting it would throw away whatever steps did complete.
/// Generic over its `Ok` type since it never actually produces one — every
/// path through this function ends in `bail!` — which lets each caller's
/// `?`/`return Err(...)` line up with whatever type that caller returns.
fn mark_failed<T>(
    root: &Path,
    id: Uuid,
    tree_path: &Path,
    log_contents: &str,
    message: &str,
) -> Result<T> {
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
/// if it's tracked it's already in the worktree. `fs::copy` overwrites an
/// existing destination, which is what makes this reusable as `wt env
/// refresh`'s re-copy. Returns the relative paths actually copied, so a
/// caller can report them.
pub(crate) fn copy_globs(
    base: &Path,
    tree_path: &Path,
    patterns: &[String],
    shared: &[String],
) -> Result<Vec<String>> {
    let mut copied = Vec::new();
    if patterns.is_empty() {
        return Ok(copied);
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
        copied.push(relpath);
    }
    Ok(copied)
}

pub fn rm_tree(root: &Path, selector: &str, force: bool, delete_branch: bool) -> Result<()> {
    let store = store::load(root)?;
    let tree = store::resolve(&store.trees, selector)?;
    let id = tree.id;
    let name = tree.name.clone();
    let branch = tree.branch.clone();
    let tree_path = tree.path.clone();
    let state = tree.state;
    let provision_pid = tree.provision_pid;
    let repo = store
        .repos
        .get(&tree.repo)
        .with_context(|| format!("tree '{name}' references unknown repo '{}'", tree.repo))?
        .clone();

    if state == TreeState::Provisioning && !force {
        bail!(
            "tree '{name}' is still provisioning; run `wt wait '{name}'` first, or pass --force \
             to stop it and remove anyway"
        );
    }

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

        // Only reachable with `force` when still provisioning (the guard
        // above already refused otherwise), so it's always correct to stop
        // the child here before the directory disappears under it.
        if state == TreeState::Provisioning {
            crate::proc::stop_provisioning_child(provision_pid, id);
        }

        let remove_result = remove_tree_dir(&tree_path);
        if tree_path.exists() {
            let err = match remove_result {
                Ok(()) => anyhow::anyhow!(
                    "removal reported success but {} is still on disk",
                    tree_path.display()
                ),
                Err(e) => e,
            };
            bail!(
                "failed to remove worktree at {}: {err:#}. The registry entry is kept; remove it \
                 by hand (rm -rf \"{}\" && git -C {} worktree prune) or run `wt doctor --fix`.",
                tree_path.display(),
                tree_path.display(),
                repo.base.display()
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

/// Deletes the tree's directory directly instead of calling `git worktree
/// remove`. wt's own dirty/unpushed guards already ran by this point, so
/// git's refusal buys nothing — and for a tree with submodules, routing
/// around that refusal the obvious way (`git submodule deinit` first) is
/// wrong: deinit rewrites `submodule.<name>.url`/`.active` in the *common*
/// `.git/config`, shared by base and every other worktree, corrupting their
/// submodule registration too. The caller's `git worktree prune` afterward
/// clears the now-stale administrative entry this leaves behind.
/// A few retries: a step that was still running when it got signalled can
/// keep a writer inside the tree for a moment after the signal is sent, and
/// that writer can lose the race with this walk by a hair.
fn remove_tree_dir(tree_path: &Path) -> Result<()> {
    let mut last_err = None;
    for attempt in 0..3 {
        match fs::remove_dir_all(tree_path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
    }
    Err(last_err.unwrap()).with_context(|| format!("removing {}", tree_path.display()))
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
            provision_pid: None,
            parent_branch: None,
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

    mod onto {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn git_cmd(args: &[&str], cwd: &Path) {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        fn head(path: &Path) -> String {
            String::from_utf8(
                Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(path)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string()
        }

        fn head_of(base: &Path, rev: &str) -> String {
            String::from_utf8(
                Command::new("git")
                    .args(["rev-parse", rev])
                    .current_dir(base)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string()
        }

        /// `master`, with a fake `origin/master` pointing at the same
        /// commit — enough for `git worktree add ... origin/master` to
        /// resolve without a real remote, since these tests only exercise
        /// `--onto` paths where `create_tree_with` never fetches.
        fn fixture() -> (PathBuf, Repo) {
            let dir = std::env::temp_dir().join(format!("wt-tree-onto-test-{}", Uuid::now_v7()));
            let base = dir.join("base");
            fs::create_dir_all(&base).unwrap();
            git_cmd(&["init", "-q", "-b", "master"], &base);
            git_cmd(&["config", "user.email", "t@t"], &base);
            git_cmd(&["config", "user.name", "t"], &base);
            fs::write(base.join("f.txt"), "0\n").unwrap();
            git_cmd(&["add", "-A"], &base);
            git_cmd(&["commit", "-qm", "init"], &base);
            let sha = head(&base);
            git_cmd(&["update-ref", "refs/remotes/origin/master", &sha], &base);
            git_cmd(&["branch", "stacked"], &base);

            let repo = Repo {
                base,
                trunk: "master".into(),
                branch_prefix: "josh/".into(),
                last_fetch: Some(Utc::now()),
                shared: Vec::new(),
                copy: Vec::new(),
                env: Default::default(),
                steps: Vec::new(),
            };
            (dir, repo)
        }

        fn fake_gt(dir: &Path, log: &Path) -> PathBuf {
            let script = dir.join("gt");
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\necho \"$* | $(pwd)\" >> \"{}\"\nexit 0\n",
                    log.display()
                ),
            )
            .unwrap();
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
            script
        }

        fn fake_gt_failing(dir: &Path, log: &Path) -> PathBuf {
            let script = dir.join("gt");
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\necho \"$* | $(pwd)\" >> \"{}\"\necho 'gt: not authenticated' >&2\nexit 1\n",
                    log.display()
                ),
            )
            .unwrap();
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
            script
        }

        fn sample_tree(id: Uuid, repo: &str, name: &str, branch: &str, path: PathBuf) -> Tree {
            Tree {
                id,
                repo: repo.into(),
                name: name.into(),
                branch: branch.into(),
                path,
                created: Utc::now(),
                state: TreeState::Ready,
                step_label: None,
                step_index: None,
                step_total: None,
                log_path: None,
                provision_pid: None,
                parent_branch: None,
            }
        }

        #[test]
        fn create_tree_with_onto_a_branch_name_sets_parent_and_tracks_with_graphite() {
            let (dir, repo) = fixture();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "next pr".into(),
                branch: None,
                onto: Some("stacked".into()),
                profiles: None,
            };
            let (id, tree_path, _) = create_tree_with(&root, &opts, gt.to_str().unwrap()).unwrap();

            assert_eq!(head(&tree_path), head_of(&repo.base, "stacked"));

            let log_contents = fs::read_to_string(&log).unwrap();
            assert!(
                log_contents.contains("track --parent stacked --no-interactive"),
                "log was: {log_contents}"
            );
            assert!(
                log_contents.contains(&tree_path.display().to_string()),
                "gt must run with cwd in the new tree; log was: {log_contents}"
            );

            let store = store::load(&root).unwrap();
            let t = store.trees.iter().find(|t| t.id == id).unwrap();
            assert_eq!(t.parent_branch.as_deref(), Some("stacked"));

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn create_tree_with_onto_a_tree_selector_uses_its_live_branch_not_the_recorded_one() {
            let (dir, repo) = fixture();
            let root = dir.join("wtroot");

            // A tree registered under a branch it no longer has checked
            // out — the drift `Tree.branch` is known to accumulate once
            // `gt` moves a tree along its stack.
            git_cmd(&["branch", "live-branch"], &repo.base);
            let other_path = dir.join("other-tree");
            git_cmd(
                &[
                    "worktree",
                    "add",
                    other_path.to_str().unwrap(),
                    "live-branch",
                ],
                &repo.base,
            );
            let other_path = fs::canonicalize(&other_path).unwrap();
            let other_id = Uuid::now_v7();

            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                s.trees.push(sample_tree(
                    other_id,
                    "r",
                    "other",
                    "recorded-branch",
                    other_path,
                ));
                Ok(())
            })
            .unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "on top of other".into(),
                branch: None,
                onto: Some("other".into()),
                profiles: None,
            };
            let (_, tree_path, _) = create_tree_with(&root, &opts, gt.to_str().unwrap()).unwrap();

            assert_eq!(head(&tree_path), head_of(&repo.base, "live-branch"));
            let log_contents = fs::read_to_string(&log).unwrap();
            assert!(
                log_contents.contains("--parent live-branch"),
                "log was: {log_contents}"
            );
            assert!(
                !log_contents.contains("recorded-branch"),
                "must use the live branch, not the recorded one; log was: {log_contents}"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn create_tree_with_onto_and_branch_both_apply() {
            let (dir, repo) = fixture();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "ignored when branch is set".into(),
                branch: Some("josh/explicit-branch".into()),
                onto: Some("stacked".into()),
                profiles: None,
            };
            let (id, tree_path, _) = create_tree_with(&root, &opts, gt.to_str().unwrap()).unwrap();

            assert_eq!(head(&tree_path), head_of(&repo.base, "stacked"));
            let store = store::load(&root).unwrap();
            let t = store.trees.iter().find(|t| t.id == id).unwrap();
            assert_eq!(t.branch, "josh/explicit-branch");
            assert_eq!(t.parent_branch.as_deref(), Some("stacked"));

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn create_tree_with_keeps_the_tree_and_warns_when_gt_track_fails() {
            let (dir, repo) = fixture();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt_failing(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "flaky".into(),
                branch: None,
                onto: Some("stacked".into()),
                profiles: None,
            };
            let (id, tree_path, _) = create_tree_with(&root, &opts, gt.to_str().unwrap())
                .expect("a failed `gt track` must not fail tree creation");

            assert!(tree_path.exists());
            assert!(
                fs::read_to_string(&log).unwrap().contains("track"),
                "gt track must still have been attempted"
            );

            let store = store::load(&root).unwrap();
            let t = store.trees.iter().find(|t| t.id == id).unwrap();
            assert_eq!(
                t.parent_branch.as_deref(),
                Some("stacked"),
                "the parent is recorded even when gt couldn't track it"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn create_tree_with_no_onto_never_calls_gt() {
            let (dir, repo) = fixture();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "plain".into(),
                branch: None,
                onto: None,
                profiles: None,
            };
            let (id, tree_path, _) = create_tree_with(&root, &opts, gt.to_str().unwrap()).unwrap();

            assert!(!log.exists(), "gt must never run without --onto");
            assert_eq!(head(&tree_path), head(&repo.base));

            let store = store::load(&root).unwrap();
            let t = store.trees.iter().find(|t| t.id == id).unwrap();
            assert!(t.parent_branch.is_none());

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn resolve_onto_prefers_a_tree_selector_over_a_branch_of_the_same_name() {
            let (dir, repo) = fixture();
            git_cmd(&["branch", "shared-name"], &repo.base);
            git_cmd(&["branch", "actual-branch"], &repo.base);
            let other_path = dir.join("other-tree");
            git_cmd(
                &[
                    "worktree",
                    "add",
                    other_path.to_str().unwrap(),
                    "actual-branch",
                ],
                &repo.base,
            );
            let other_path = fs::canonicalize(&other_path).unwrap();
            let tree = sample_tree(
                Uuid::now_v7(),
                "r",
                "shared-name",
                "shared-name",
                other_path,
            );

            let mut store = store::Store::default();
            store.repos.insert("r".into(), repo.clone());
            store.trees = vec![tree];

            let resolved = resolve_onto(&store, "r", &repo.base, "shared-name").unwrap();
            assert_eq!(resolved, "actual-branch");

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn resolve_onto_ambiguous_tree_name_lists_candidates() {
            let (dir, repo) = fixture();
            let tree_a = sample_tree(
                Uuid::now_v7(),
                "r",
                "foo bar",
                "b1",
                PathBuf::from("/nonexistent-a"),
            );
            let tree_b = sample_tree(
                Uuid::now_v7(),
                "r",
                "foo baz",
                "b2",
                PathBuf::from("/nonexistent-b"),
            );
            let mut store = store::Store::default();
            store.repos.insert("r".into(), repo.clone());
            store.trees = vec![tree_a, tree_b];

            let err = resolve_onto(&store, "r", &repo.base, "foo").unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("ambiguous"), "message was: {msg}");
            assert!(msg.contains("foo bar"));
            assert!(msg.contains("foo baz"));

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn resolve_onto_falls_back_to_a_branch_name_when_no_tree_matches() {
            let (dir, repo) = fixture();
            let store = store::Store::default();

            let resolved = resolve_onto(&store, "r", &repo.base, "stacked").unwrap();
            assert_eq!(resolved, "stacked");

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn resolve_onto_falls_back_to_a_commit_ish_when_nothing_named_it_matches() {
            let (dir, repo) = fixture();
            let sha = head(&repo.base);
            let store = store::Store::default();

            let resolved = resolve_onto(&store, "r", &repo.base, &sha).unwrap();
            assert_eq!(resolved, sha);

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn resolve_onto_errors_when_nothing_matches() {
            let (dir, repo) = fixture();
            let store = store::Store::default();

            let err = resolve_onto(&store, "r", &repo.base, "does-not-exist-anywhere").unwrap_err();
            assert!(
                err.to_string()
                    .contains("matches no tree, branch, or commit")
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn resolve_onto_only_considers_trees_in_the_same_repo() {
            let (dir, repo) = fixture();
            // A tree of the same name lives in a different repo; it must
            // not satisfy `--onto` here, since its branch may not even
            // exist in this repo's history.
            let tree = sample_tree(
                Uuid::now_v7(),
                "other-repo",
                "stacked",
                "stacked",
                PathBuf::from("/nonexistent"),
            );
            let mut store = store::Store::default();
            store.repos.insert("r".into(), repo.clone());
            store.trees = vec![tree];

            // Falls through the (empty, in-repo) tree tier straight to the
            // branch-name tier, resolving the local branch `stacked`
            // instead of erroring or reading the other repo's tree.
            let resolved = resolve_onto(&store, "r", &repo.base, "stacked").unwrap();
            assert_eq!(resolved, "stacked");

            fs::remove_dir_all(&dir).ok();
        }
    }
}
