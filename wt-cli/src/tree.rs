use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

use crate::config;
use crate::git;
use crate::store::{self, Repo, Tree, TreeState};

const FETCH_STALE_AFTER: chrono::Duration = chrono::Duration::minutes(5);

pub struct NewOptions {
    pub repo: String,
    pub name: String,
    pub branch: Option<String>,
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

/// The result of `plan_tree`: every decision `wt tree new` has to make before it
/// touches disk, resolved once so both the claim path and the cold path act
/// on the same answer.
pub(crate) struct TreePlan {
    pub(crate) repo_name: String,
    pub(crate) repo: Repo,
    pub(crate) name: String,
    pub(crate) branch: String,
    /// Resolved to a concrete commit, not a ref — this is what a claim
    /// compares a spare's HEAD against, and what the cold path branches
    /// from.
    pub(crate) start_point: String,
    pub(crate) profiles: Option<Vec<String>>,
}

pub fn new_tree(root: &Path, config_path: &Path, opts: NewOptions) -> Result<PathBuf> {
    let config = config::load(config_path)?;
    let plan = plan_tree(root, &config, &opts)?;
    let claimed = crate::spare::claim(root, &plan)?;
    let (id, tree_path, needs_steps) = match claimed {
        Some(c) => (c.id, c.path, c.needs_steps),
        None => {
            let (id, tree_path, _) = create_cold(root, &plan)?;
            (id, tree_path, true)
        }
    };
    if needs_steps {
        start_provisioning(root, config_path, id, &plan.profiles)?;
    }
    println!("{}", tree_path.display());
    // A claimed or freshly built tree both leave the repo's spare pool one
    // short; topping up here is what keeps the next `wt tree new` fast too. Never
    // lets a spare-provisioning hiccup fail the command that just succeeded.
    crate::spare::top_up(root, config_path, Some(&plan.repo_name)).ok();
    Ok(tree_path)
}

/// Resolves `--onto`, fetches trunk when stale, derives the branch name, and
/// rejects a collision — everything `wt tree new` needs to decide before it
/// claims a spare or builds cold. Branch validation happens here, ahead of
/// any claim, so a colliding name never consumes a spare only to fail anyway.
fn plan_tree(root: &Path, config: &config::Config, opts: &NewOptions) -> Result<TreePlan> {
    let store = store::load(root)?;
    let repo = store.repos.get(&opts.repo).cloned().with_context(|| {
        format!(
            "unknown repo '{}'. Known repos: {}",
            opts.repo,
            known_repos(&store)
        )
    })?;
    let repo_config = config::repo(config, &opts.repo)?.clone();

    let onto = opts
        .onto
        .as_deref()
        .map(|sel| resolve_onto(&store, &opts.repo, &repo.base, sel))
        .transpose()?;

    // A ref resolved by `--onto` is used as-is; only the trunk-based path
    // needs a fresh `origin/<trunk>` to branch from.
    if onto.is_none() {
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
            format!("{}{}", repo_config.branch_prefix, slug)
        }
    };
    if git::branch_exists_local(&repo.base, &branch)? {
        bail!("branch '{branch}' already exists locally");
    }
    if git::branch_exists_remote(&repo.base, &branch)? {
        bail!("branch '{branch}' already exists on origin");
    }

    let start_point_ref = onto.unwrap_or_else(|| format!("origin/{}", repo_config.trunk));
    let start_point = git::rev_parse(&repo.base, &start_point_ref)
        .with_context(|| format!("resolving {start_point_ref}"))?;

    Ok(TreePlan {
        repo_name: opts.repo.clone(),
        repo,
        name: opts.name.clone(),
        branch,
        start_point,
        profiles: opts.profiles.clone(),
    })
}

/// Worktree creation, shared-state wiring, and the registry write — stops
/// short of starting provisioning so `wt repo lift` can pop its stash into the
/// tree first, before a background install could touch any of the same
/// files, and so `new_tree` can start it only when `needs_steps` says so.
fn create_cold(root: &Path, plan: &TreePlan) -> Result<(Uuid, PathBuf, PathBuf)> {
    let id = Uuid::now_v7();
    let repo_dir = root.join(&plan.repo_name);
    let tree_path = repo_dir.join("trees").join(id.to_string());
    git::worktree_add(&plan.repo.base, &tree_path, &plan.branch, &plan.start_point)?;
    let tree_path = finish_worktree_checkout(&tree_path)?;
    let log_path = register_and_wire(
        root,
        id,
        Registration {
            repo_dir: &repo_dir,
            base: &plan.repo.base,
            repo_name: &plan.repo_name,
            name: &plan.name,
            branch: &plan.branch,
            tree_path: &tree_path,
        },
    )?;
    Ok((id, tree_path, log_path))
}

/// Clears an inherited `core.hooksPath` and resolves the checkout's
/// canonical path — every worktree-creation route needs both done before
/// registering it, whether its branch is new or already existed.
fn finish_worktree_checkout(tree_path: &Path) -> Result<PathBuf> {
    if let Err(e) = git::clear_worktree_hooks_path(tree_path) {
        eprintln!("warning: could not clear inherited worktree hooksPath: {e:#}");
    }
    Ok(fs::canonicalize(tree_path)?)
}

struct Registration<'a> {
    repo_dir: &'a Path,
    base: &'a Path,
    repo_name: &'a str,
    name: &'a str,
    branch: &'a str,
    tree_path: &'a Path,
}

/// Registers before wiring so a failure remains visible to inspection and cleanup.
fn register_and_wire(root: &Path, id: Uuid, registration: Registration<'_>) -> Result<PathBuf> {
    let Registration {
        repo_dir,
        base,
        repo_name,
        name,
        branch,
        tree_path,
    } = registration;
    let log_path = tree_path.join(crate::repo::PROVISION_LOG_NAME);
    let now = Utc::now();
    store::with_store_lock(root, |s| {
        s.trees.push(Tree {
            id,
            repo: repo_name.to_string(),
            name: name.to_string(),
            branch: branch.to_string(),
            path: tree_path.to_path_buf(),
            created: now,
            state: TreeState::Provisioning,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: Some(log_path.clone()),
            provision_pid: None,
            spare: false,
        });
        Ok(())
    })?;

    if let Err(e) = wire_fresh_checkout(repo_dir, base, tree_path) {
        return Err(mark_failed::<()>(
            root,
            id,
            tree_path,
            &format!("wiring shared state failed:\n{e:#}\n"),
            "wiring shared state failed",
        )
        .unwrap_err());
    }

    Ok(log_path)
}

/// Resolves `--onto`'s selector into the branch `wt tree new` should create its
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

/// Where `branch` is checked out right now, if anywhere, paired with that
/// directory. `None` when no worktree has it, so a caller cannot be handed
/// a `cd` command with nowhere to point it.
fn holder_of_branch(
    store: &store::Store,
    repo_name: &str,
    repo: &Repo,
    branch: &str,
) -> Option<(String, PathBuf)> {
    let base = fs::canonicalize(&repo.base).unwrap_or_else(|_| repo.base.clone());
    let worktrees = git::worktree_branches(&repo.base).ok()?;
    let (path, _) = worktrees
        .iter()
        .find(|(_, b)| b.as_deref() == Some(branch))?;
    if *path == base {
        return Some(("the repo's base checkout".to_string(), path.clone()));
    }
    if let Some(t) = store
        .trees
        .iter()
        .find(|t| t.repo == repo_name && &t.path == path)
    {
        return Some((format!("tree \"{}\"", t.name), path.clone()));
    }
    Some((
        format!("an unregistered worktree at {}", path.display()),
        path.clone(),
    ))
}

fn start_provisioning(
    root: &Path,
    config_path: &Path,
    id: Uuid,
    profiles: &Option<Vec<String>>,
) -> Result<()> {
    let pid = spawn_background_provisioning(root, config_path, id, profiles)?;
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
/// be popped straight into the tree `wt tree new` just created from it, no patch
/// file needed.
///
/// No `--onto` here: the stash was taken against whatever base's `HEAD`
/// already was, so replaying it onto a different branch's tip is a rebase
/// this function doesn't do, and popping it there would surface as merge
/// conflicts with no indication that the mismatch is the real cause.
pub fn adopt(root: &Path, config_path: &Path, opts: AdoptOptions) -> Result<PathBuf> {
    let store = store::load(root)?;
    let (repo_name, repo) = resolve_adopt_repo(&store, opts.repo)?;

    if !git::is_dirty(&repo.base)? {
        bail!(
            "{repo_name}'s base ({}) is clean; there is nothing to lift",
            repo.base.display()
        );
    }

    let stash_sha =
        git::stash_push_include_untracked(&repo.base, &format!("wt repo lift: {}", opts.name))?;

    let new_opts = NewOptions {
        repo: repo_name.clone(),
        name: opts.name.clone(),
        branch: opts.branch.clone(),
        onto: None,
        profiles: opts.profiles.clone(),
    };
    let config = config::load(config_path)?;
    let plan = plan_tree(root, &config, &new_opts).map_err(|e| {
        anyhow::anyhow!(
            "adopted work is stashed in {repo_name}'s base; planning the tree failed: {e:#}\n\
             recover it with: git -C {} stash pop",
            repo.base.display()
        )
    })?;
    let (id, tree_path, _log_path) = create_cold(root, &plan).map_err(|e| {
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

    start_provisioning(root, config_path, id, &opts.profiles)?;
    println!("{}", tree_path.display());
    Ok(tree_path)
}

/// `Some(name)` looks the repo up by name; `None` resolves from the current
/// directory, since `wt repo lift` is meant to be run from inside the base
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

pub struct AdoptBranchOptions {
    pub repo: Option<String>,
    pub branch: String,
    pub name: Option<String>,
    pub profiles: Option<Vec<String>>,
}

/// Materializes a tree for a branch that already exists. It never creates
/// a branch; it checks out the existing Git branch in a new worktree.
pub fn adopt_branch(root: &Path, config_path: &Path, opts: AdoptBranchOptions) -> Result<PathBuf> {
    let store = store::load(root)?;
    let (repo_name, repo) = resolve_branch_repo(&store, opts.repo.as_deref())?;
    let repo_config = config::repo(&config::load(config_path)?, &repo_name)?.clone();

    if !git::branch_exists_local(&repo.base, &opts.branch)? {
        bail!(
            "branch '{}' does not exist locally in '{repo_name}'",
            opts.branch
        );
    }
    if let Some((holder, path)) = holder_of_branch(&store, &repo_name, &repo, &opts.branch) {
        bail!(
            "branch '{}' is already checked out in {holder} ({})",
            opts.branch,
            path.display()
        );
    }
    if let Some(t) = store
        .trees
        .iter()
        .find(|t| t.repo == repo_name && !t.spare && t.branch == opts.branch)
    {
        bail!(
            "tree '{}' already claims branch '{}'; nothing to adopt",
            t.name,
            opts.branch
        );
    }

    let name = opts.name.clone().unwrap_or_else(|| {
        opts.branch
            .strip_prefix(&repo_config.branch_prefix)
            .unwrap_or(&opts.branch)
            .to_string()
    });

    let id = Uuid::now_v7();
    let repo_dir = root.join(&repo_name);
    let tree_path = repo_dir.join("trees").join(id.to_string());
    git::worktree_add_existing(&repo.base, &tree_path, &opts.branch)?;
    let tree_path = finish_worktree_checkout(&tree_path)?;

    register_and_wire(
        root,
        id,
        Registration {
            repo_dir: &repo_dir,
            base: &repo.base,
            repo_name: &repo_name,
            name: &name,
            branch: &opts.branch,
            tree_path: &tree_path,
        },
    )?;

    start_provisioning(root, config_path, id, &opts.profiles)?;
    println!("{}", tree_path.display());
    crate::spare::top_up(root, config_path, Some(&repo_name)).ok();
    Ok(tree_path)
}

/// `--repo`, when given; otherwise the repository containing the current
/// directory, resolved the same way most tree selectors are — a nested
/// tree's own repo wins over an enclosing one.
fn resolve_branch_repo(store: &store::Store, repo: Option<&str>) -> Result<(String, Repo)> {
    if let Some(name) = repo {
        let repo = store.repos.get(name).cloned().with_context(|| {
            format!("unknown repo '{name}'. Known repos: {}", known_repos(store))
        })?;
        return Ok((name.to_string(), repo));
    }

    let cwd = std::env::current_dir().context("reading current directory")?;
    let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
    let name = store::repo_for_cwd(store, &cwd)
        .context("current directory is not inside a registered repo; pass --repo")?
        .to_string();
    let repo = store.repos[&name].clone();
    Ok((name, repo))
}

/// Re-execs the binary as `wt __provision`, detached, so the parent can
/// return the tree path immediately and the OS reparents the child rather
/// than a live handle pinning it to a process that is about to go away.
fn spawn_background_provisioning(
    root: &Path,
    config_path: &Path,
    id: Uuid,
    profiles: &Option<Vec<String>>,
) -> Result<u32> {
    let mut args = vec!["__provision".to_string(), id.to_string()];
    if let Some(profiles) = profiles {
        args.push("--profile".to_string());
        args.push(profiles.join(","));
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::proc::spawn_detached(root, config_path, &arg_refs)
}

/// Leaves the tree on disk and registered as `Failed` rather than cleaning
/// up — a half-provisioned tree is still worth inspecting or resuming by
/// hand, and deleting it would throw away whatever steps did complete.
/// Generic over its `Ok` type since it never actually produces one — every
/// path through this function ends in `bail!` — which lets each caller's
/// `?`/`return Err(...)` line up with whatever type that caller returns.
pub(crate) fn mark_failed<T>(
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

/// Symlinks each shared path into a fresh checkout and copies its env-glob
/// files in from base — everything a brand new worktree needs wired before
/// any provisioning step runs, whether it becomes an ordinary tree right
/// away or sits as a hot spare until claimed. `shared`/`copy` come from
/// `.worktreeinclude` read fresh here, not from anything persisted, so an
/// edit to that manifest takes effect on the next tree without a re-init.
pub(crate) fn wire_fresh_checkout(repo_dir: &Path, base: &Path, tree_path: &Path) -> Result<()> {
    let (shared, copy) = crate::repo::parse_worktreeinclude(base)?;
    wire_shared_symlinks(&repo_dir.join("shared"), tree_path, &shared)?;
    copy_globs(base, tree_path, &copy, &shared)?;
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
/// existing destination, which is what makes this reusable as `wt tree env`'s
/// re-copy. Returns the relative paths actually copied, so a
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

pub fn rm_tree(
    root: &Path,
    config_path: &Path,
    selector: &str,
    force: bool,
    delete_branch: bool,
) -> Result<()> {
    let store = store::load(root)?;
    let tree = store::resolve(&store.trees, selector)?;
    let id = tree.id;
    let name = tree.name.clone();
    let branch = store::live_branch(tree).unwrap_or_else(|| tree.branch.clone());
    let tree_path = tree.path.clone();
    let state = tree.state;
    let provision_pid = tree.provision_pid;
    let repo = store
        .repos
        .get(&tree.repo)
        .with_context(|| format!("tree '{name}' references unknown repo '{}'", tree.repo))?
        .clone();
    let config = config::load(config_path)?;
    let repo_config = config::repo(&config, &tree.repo)?.clone();

    if state == TreeState::Provisioning && !force {
        bail!(
            "tree '{name}' is still provisioning; run `wt tree wait '{name}'` first, or pass --force \
             to stop it and remove anyway"
        );
    }

    let unsaved = branch_has_unsaved_commits(&repo.base, &branch, &repo_config.trunk)?;

    if tree_path.exists() {
        if !force {
            if git::is_dirty(&tree_path)? {
                bail!("tree '{name}' has uncommitted changes; use --force to remove anyway");
            }
            if unsaved {
                bail!(
                    "tree '{name}' has commits that are neither pushed nor landed on \
                     origin/{}; use --force to remove anyway",
                    repo_config.trunk
                );
            }
        }

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
                 by hand (rm -rf \"{}\" && git -C {} worktree prune) or run `wt upkeep doctor --fix`.",
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
        if unsaved {
            eprintln!(
                "keeping branch '{branch}': it has commits that are neither pushed nor landed \
                 on origin/{}",
                repo_config.trunk
            );
        } else if let Err(e) = git::delete_branch(&repo.base, &branch) {
            eprintln!("warning: could not delete branch '{branch}': {e}");
        }
    }

    crate::spare::top_up(root, config_path, Some(&tree.repo)).ok();
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
pub(crate) fn remove_tree_dir(tree_path: &Path) -> Result<()> {
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

fn branch_has_unsaved_commits(base: &Path, branch: &str, trunk: &str) -> Result<bool> {
    // Nothing ahead of trunk at all: the common case, and cheaper than the
    // patch-id walk below.
    if !git::commits_ahead(base, &format!("origin/{trunk}..{branch}"))? {
        return Ok(false);
    }
    // A squash merge lands the work under a new SHA, so compare patch-ids:
    // commits already on trunk are not at risk however far ahead they look.
    if git::unlanded_commits(base, &format!("origin/{trunk}"), branch)?.is_empty() {
        return Ok(false);
    }
    match git::branch_upstream(base, branch) {
        Some(upstream) => Ok(!git::unlanded_commits(base, &upstream, branch)?.is_empty()),
        None => Ok(true),
    }
}

pub struct GcOptions {
    pub repo: Option<String>,
    pub dry_run: bool,
}

pub fn gc(root: &Path, config_path: &Path, opts: GcOptions) -> Result<()> {
    let store = store::load(root)?;
    let config = config::load(config_path)?;
    let mut candidates = 0;

    for tree in &store.trees {
        if opts.repo.as_deref().is_some_and(|repo| repo != tree.repo) {
            continue;
        }
        let Some(repo) = store.repos.get(&tree.repo) else {
            eprintln!(
                "skipping '{}': repo '{}' is not registered",
                tree.name, tree.repo
            );
            continue;
        };
        let repo_config = match config::repo(&config, &tree.repo) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("skipping '{}': {e:#}", tree.name);
                continue;
            }
        };
        match gc_verdict(repo, repo_config, tree) {
            Ok(Some(reason)) => {
                eprintln!("skipping '{}': {reason}", tree.name);
                continue;
            }
            Err(e) => {
                eprintln!("skipping '{}': {e:#}", tree.name);
                continue;
            }
            Ok(None) => {}
        }

        candidates += 1;
        if opts.dry_run {
            println!("would reap '{}' ({})", tree.name, tree.path.display());
            continue;
        }
        println!("reaping '{}' ({})", tree.name, tree.path.display());
        if let Err(e) = rm_tree(root, config_path, &tree.id.to_string(), false, true) {
            eprintln!("failed to reap '{}': {e:#}", tree.name);
        }
    }

    if candidates == 0 {
        println!("nothing to reap");
    }
    Ok(())
}

/// `None` means a tree is safe to reap. The landed check stays in step with
/// `rm_tree`: gc hands each selected tree to that same safeguard.
fn gc_verdict(
    repo: &Repo,
    repo_config: &config::RepoConfig,
    tree: &Tree,
) -> Result<Option<String>> {
    if tree.spare {
        return Ok(Some("hot spare".to_string()));
    }
    if tree.state == TreeState::Provisioning {
        return Ok(Some("still provisioning".to_string()));
    }
    if tree.state == TreeState::Failed {
        return Ok(Some(
            "provisioning failed; read its log, then remove it with `wt tree rm`".to_string(),
        ));
    }
    if tree.path.exists() && git::is_dirty(&tree.path)? {
        return Ok(Some("uncommitted changes".to_string()));
    }
    let branch = store::live_branch(tree).unwrap_or_else(|| tree.branch.clone());
    if git::commits_ahead(
        &repo.base,
        &format!("origin/{}..{branch}", repo_config.trunk),
    )? {
        let unlanded = git::unlanded_commits(
            &repo.base,
            &format!("origin/{}", repo_config.trunk),
            &branch,
        )?;
        if !unlanded.is_empty() {
            let n = unlanded.len();
            return Ok(Some(format!(
                "{n} commit{} not yet in origin/{}",
                if n == 1 { "" } else { "s" },
                repo_config.trunk
            )));
        }
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
            .filter(|tree| &tree.repo == repo_name)
            .collect();
        let mut stale_ids = Vec::new();

        for tree in &registered {
            if !tree.path.exists() {
                println!(
                    "  stale registry entry: '{}' — {} no longer exists",
                    tree.name,
                    tree.path.display()
                );
                stale_ids.push(tree.id);
            }
        }

        for worktree in &worktrees {
            if worktree.path == repo.base {
                continue;
            }
            if !registered.iter().any(|tree| tree.path == worktree.path) {
                let branch = worktree.branch.as_deref().unwrap_or("(detached)");
                println!(
                    "  unregistered worktree: {} (branch {branch}) — not tracked by wt; leave it \
                     alone or register it by hand if you want wt to manage it",
                    worktree.path.display()
                );
            }
        }

        for tree in &registered {
            if !tree.path.exists() {
                continue;
            }
            match worktrees.iter().find(|worktree| worktree.path == tree.path) {
                Some(worktree)
                    if !tree.spare && worktree.branch.as_deref() != Some(tree.branch.as_str()) =>
                {
                    let actual = worktree.branch.as_deref().unwrap_or("(detached)");
                    println!(
                        "  branch mismatch: '{}' registered as '{}' but checked out as {actual}",
                        tree.name, tree.branch
                    );
                    if opts.fix
                        && let Some(actual_branch) = &worktree.branch
                    {
                        fix_branch_mismatch(root, tree.id, actual_branch)?;
                        println!(
                            "    fixed: '{}' now registered as '{actual_branch}'",
                            tree.name
                        );
                    }
                }
                Some(_) => {}
                None => println!(
                    "  drifted: '{}' exists on disk at {} but git no longer lists it as a worktree",
                    tree.name,
                    tree.path.display()
                ),
            }
        }

        if opts.fix {
            if !stale_ids.is_empty() {
                let n = stale_ids.len();
                store::with_store_lock(root, |store| {
                    store.trees.retain(|tree| !stale_ids.contains(&tree.id));
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

fn fix_branch_mismatch(root: &Path, tree_id: Uuid, live_branch: &str) -> Result<()> {
    store::with_store_lock(root, |store| {
        if let Some(tree) = store.trees.iter_mut().find(|tree| tree.id == tree_id) {
            tree.branch = live_branch.to_string();
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repo() -> PathBuf {
        let path = std::env::temp_dir().join(format!("wt-tree-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["config", "user.email", "test@example.com"]);
        git(&path, &["config", "user.name", "Test"]);
        fs::write(path.join("file"), "initial\n").unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-qm", "initial"]);
        path
    }

    #[test]
    fn slugify_makes_branch_suffixes() {
        assert_eq!(slugify("Fix the Login!"), "fix-the-login");
        assert_eq!(slugify("***"), "");
    }

    #[test]
    fn resolve_onto_accepts_a_local_branch_and_commit() {
        let base = repo();
        git(&base, &["branch", "feature"]);
        let store = store::Store::default();

        assert_eq!(
            resolve_onto(&store, "repo", &base, "feature").unwrap(),
            "feature"
        );
        assert_eq!(resolve_onto(&store, "repo", &base, "HEAD").unwrap(), "HEAD");

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn resolve_onto_rejects_an_unknown_ref() {
        let base = repo();
        let err = resolve_onto(&store::Store::default(), "repo", &base, "missing").unwrap_err();
        assert!(
            err.to_string()
                .contains("matches no tree, branch, or commit")
        );
        fs::remove_dir_all(base).ok();
    }
}
