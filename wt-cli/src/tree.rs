use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

use crate::config;
use crate::git;
use crate::restack;
use crate::stack;
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

/// The result of `plan_tree`: every decision `wt new` has to make before it
/// touches disk, resolved once so both the claim path and the cold path act
/// on the same answer.
pub(crate) struct TreePlan {
    pub(crate) repo_name: String,
    pub(crate) repo: Repo,
    pub(crate) repo_config: config::RepoConfig,
    pub(crate) name: String,
    pub(crate) branch: String,
    /// Resolved to a concrete commit, not a ref — this is what a claim
    /// compares a spare's HEAD against, and what the cold path branches
    /// from.
    pub(crate) start_point: String,
    pub(crate) parent_branch: Option<String>,
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
    // short; topping up here is what keeps the next `wt new` fast too. Never
    // lets a spare-provisioning hiccup fail the command that just succeeded.
    crate::spare::top_up(root, config_path, Some(&plan.repo_name)).ok();
    Ok(tree_path)
}

/// Resolves `--onto`, fetches trunk when stale, derives the branch name, and
/// rejects a collision — everything `wt new` needs to decide before it
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
            format!("{}{}", repo_config.branch_prefix, slug)
        }
    };
    if git::branch_exists_local(&repo.base, &branch)? {
        bail!("branch '{branch}' already exists locally");
    }
    if git::branch_exists_remote(&repo.base, &branch)? {
        bail!("branch '{branch}' already exists on origin");
    }

    let start_point_ref = parent_branch
        .clone()
        .unwrap_or_else(|| format!("origin/{}", repo_config.trunk));
    let start_point = git::rev_parse(&repo.base, &start_point_ref)
        .with_context(|| format!("resolving {start_point_ref}"))?;

    Ok(TreePlan {
        repo_name: opts.repo.clone(),
        repo,
        repo_config,
        name: opts.name.clone(),
        branch,
        start_point,
        parent_branch,
        profiles: opts.profiles.clone(),
    })
}

/// Worktree creation, shared-state wiring, and the registry write — stops
/// short of starting provisioning so `wt adopt` can pop its stash into the
/// tree first, before a background install could touch any of the same
/// files, and so `new_tree` can start it only when `needs_steps` says so.
fn create_cold(root: &Path, plan: &TreePlan) -> Result<(Uuid, PathBuf, PathBuf)> {
    create_cold_with(root, plan, "gt")
}

fn create_cold_with(
    root: &Path,
    plan: &TreePlan,
    gt_bin: &str,
) -> Result<(Uuid, PathBuf, PathBuf)> {
    let id = Uuid::now_v7();
    let repo_dir = root.join(&plan.repo_name);
    let tree_path = repo_dir.join("trees").join(id.to_string());
    git::worktree_add(&plan.repo.base, &tree_path, &plan.branch, &plan.start_point)?;
    if let Err(e) = git::clear_worktree_hooks_path(&tree_path) {
        eprintln!("warning: could not clear inherited worktree hooksPath: {e:#}");
    }
    let tree_path = fs::canonicalize(&tree_path)?;
    if let Some(parent) = &plan.parent_branch {
        let store = store::load(root)?;
        let ctx = RepoCtx {
            name: &plan.repo_name,
            repo: &plan.repo,
            config: &plan.repo_config,
        };
        track_with_graphite(&store, &ctx, &plan.name, &tree_path, parent, gt_bin);
    }
    let log_path = tree_path.join(crate::repo::PROVISION_LOG_NAME);

    // Registered while still `Provisioning` so a failure in wiring or a
    // step below lands as a `Failed` entry, not an orphan invisible to
    // `wt ls`/`wt rm`.
    let now = Utc::now();
    store::with_store_lock(root, |s| {
        s.trees.push(Tree {
            id,
            repo: plan.repo_name.clone(),
            name: plan.name.clone(),
            branch: plan.branch.clone(),
            path: tree_path.clone(),
            created: now,
            state: TreeState::Provisioning,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: Some(log_path.clone()),
            provision_pid: None,
            parent_branch: plan.parent_branch.clone(),
            spare: false,
        });
        Ok(())
    })?;

    if let Err(e) = wire_fresh_checkout(&repo_dir, &plan.repo.base, &tree_path) {
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

/// Test seam for exercising `plan_tree` and `create_cold_with` together
/// against a stubbed `gt` binary.
#[cfg(test)]
fn create_tree_with(
    root: &Path,
    config_path: &Path,
    opts: &NewOptions,
    gt_bin: &str,
) -> Result<(Uuid, PathBuf, PathBuf)> {
    let config = config::load(config_path)?;
    let plan = plan_tree(root, &config, opts)?;
    create_cold_with(root, &plan, gt_bin)
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

/// `gt`'s wording when `--parent` names a branch it doesn't track itself —
/// the common case the first time anything stacks onto a plain `wt new`
/// tree, since that tree was never tracked. Matched loosely, like
/// `restack.rs`'s misrouted-worktree marker: a reworded message only costs
/// this specific remedy, never turns the generic warning into no warning.
const UNTRACKED_PARENT_MARKER: &str = "Cannot perform this operation on untracked branch";

/// Where `branch` is checked out right now, if anywhere, paired with that
/// directory — independent of Graphite, since the branch in question is by
/// definition one it doesn't track. `None` when no worktree has it, so a
/// caller can't be handed a `cd` command with nowhere to point it.
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

/// A repo's identity, state, and config together — grouped so the
/// functions that need all three carry one reference instead of three.
struct RepoCtx<'a> {
    name: &'a str,
    repo: &'a Repo,
    config: &'a config::RepoConfig,
}

/// Tracks the new tree's branch with Graphite, recording `parent` as its
/// parent. A failure here only warns: the tree already exists and is fully
/// usable, just outside Graphite's stack until `gt track` is run by hand —
/// far better than discarding a freshly created tree over a `gt` hiccup.
fn track_with_graphite(
    store: &store::Store,
    ctx: &RepoCtx,
    new_tree_name: &str,
    tree_path: &Path,
    parent: &str,
    gt_bin: &str,
) {
    let out = match Command::new(gt_bin)
        .args(["track", "--parent", parent, "--no-interactive"])
        .current_dir(tree_path)
        .output()
    {
        Ok(out) if out.status.success() => return,
        Ok(out) => out,
        Err(e) => {
            eprintln!(
                "warning: could not run `gt track --parent {parent}` in {}: {e:#}",
                tree_path.display()
            );
            return;
        }
    };
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprintln!(
        "{}",
        track_failure_message(store, ctx, new_tree_name, tree_path, parent, &stderr)
    );
}

/// The warning `track_with_graphite` prints on a failed `gt track`. Pulled
/// out as a pure function so the untracked-parent remedy is testable
/// without capturing stderr — the same reason `restack.rs`'s
/// misrouted-worktree check gets its own pure `misrouted_worktree` function.
fn track_failure_message(
    store: &store::Store,
    ctx: &RepoCtx,
    new_tree_name: &str,
    tree_path: &Path,
    parent: &str,
    stderr: &str,
) -> String {
    if stderr.contains(UNTRACKED_PARENT_MARKER)
        && let Some((holder, holder_dir)) = holder_of_branch(store, ctx.name, ctx.repo, parent)
    {
        return format!(
            "warning: `gt track --parent {parent}` failed because '{parent}' isn't tracked by \
             Graphite yet — track it first, in {holder}:\n  cd {} && gt track --parent {} \
             --no-interactive\nthen finish this tree, in \"{new_tree_name}\":\n  cd {} && gt \
             track --parent {parent} --no-interactive\n{}",
            holder_dir.display(),
            ctx.config.trunk,
            tree_path.display(),
            stderr.trim()
        );
    }

    format!(
        "warning: `gt track --parent {parent}` failed; the tree exists but Graphite doesn't \
         know its parent yet — fix it by hand with `gt track --parent {parent}` in {}:\n{}",
        tree_path.display(),
        stderr.trim()
    )
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
/// be popped straight into the tree `wt new` just created from it, no patch
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

pub fn rm_tree(
    root: &Path,
    config_path: &Path,
    selector: &str,
    force: bool,
    delete_branch: bool,
    reparent_children: bool,
) -> Result<()> {
    rm_tree_with(
        root,
        config_path,
        selector,
        force,
        delete_branch,
        reparent_children,
        "gt",
    )
}

fn rm_tree_with(
    root: &Path,
    config_path: &Path,
    selector: &str,
    force: bool,
    delete_branch: bool,
    reparent_children: bool,
    gt_bin: &str,
) -> Result<()> {
    let store = store::load(root)?;
    let tree = store::resolve(&store.trees, selector)?;
    let id = tree.id;
    let name = tree.name.clone();
    // The live branch, not `tree.branch`: `gt create` moves a tree without
    // updating the registry, so the recorded branch can drift and go stale.
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
            "tree '{name}' is still provisioning; run `wt wait '{name}'` first, or pass --force \
             to stop it and remove anyway"
        );
    }

    if delete_branch && !force {
        guard_stacked_children(
            &store,
            &tree.repo,
            &repo,
            &repo_config,
            &branch,
            reparent_children,
            gt_bin,
        )?;
    }

    let unsaved = branch_has_unsaved_commits(&repo.base, &branch, &repo_config.trunk)?;

    // A path that's already gone is drift, not a removal to perform: there
    // is nothing left to protect by refusing, so it skips the dirty/unpushed
    // guard entirely and goes straight to unregistering below.
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

/// Graphite children of `branch`, each paired with where it lives — the same
/// holder resolution `wt restack` uses, so a message or a `gt track` call
/// names an actual tree rather than a raw worktree path. `Ok(None)` when
/// Graphite's stack graph can't be read, or doesn't track `branch` at all:
/// there is nothing to check a delete against, so callers treat that the
/// same as "no children."
fn stacked_children(
    store: &store::Store,
    repo_name: &str,
    repo: &Repo,
    branch: &str,
) -> Result<Option<(Option<String>, Vec<restack::Step>)>> {
    let Some(stacks) = stack::load(repo_name, repo, store)? else {
        return Ok(None);
    };
    let Some(node) = stacks.graph.get(branch) else {
        return Ok(None);
    };
    let children = node
        .children
        .iter()
        .filter_map(|c| stacks.get(c))
        .map(|entry| restack::step_for(entry, store, repo))
        .collect();
    Ok(Some((node.parent.clone(), children)))
}

/// Refuses to delete a branch that Graphite still has children stacked on —
/// deleting it would orphan them — unless `reparent_children` re-parents
/// each one onto the deleted branch's own parent (trunk, if it has none)
/// with `gt track` first. Silently allows the delete when Graphite's stack
/// graph can't be read at all: there is nothing to check against, and a
/// schema change or a missing database must never block `wt rm`.
fn guard_stacked_children(
    store: &store::Store,
    repo_name: &str,
    repo: &Repo,
    repo_config: &config::RepoConfig,
    branch: &str,
    reparent_children: bool,
    gt_bin: &str,
) -> Result<()> {
    let Some((parent, children)) = stacked_children(store, repo_name, repo, branch)? else {
        return Ok(());
    };
    if children.is_empty() {
        return Ok(());
    }
    let new_parent = parent.unwrap_or_else(|| repo_config.trunk.clone());

    if !reparent_children {
        let s = if children.len() == 1 { "" } else { "es" };
        let mut msg = format!(
            "refusing to delete branch '{branch}': Graphite has {} branch{s} stacked on top of \
             it, and deleting it would orphan {}:\n",
            children.len(),
            if children.len() == 1 { "it" } else { "them" },
        );
        for step in &children {
            msg.push_str(&format!(
                "  '{}' in {}\n",
                step.branch,
                step.location.label()
            ));
        }
        msg.push_str(&format!(
            "re-parent {} onto '{new_parent}' first, then delete '{branch}' — by hand with `gt \
             track --parent {new_parent}` in each tree above, or pass --reparent-children to let \
             `wt rm` do it for you",
            if children.len() == 1 { "it" } else { "them" }
        ));
        bail!(msg);
    }

    for step in &children {
        let out = Command::new(gt_bin)
            .args([
                "track",
                &step.branch,
                "--parent",
                &new_parent,
                "--no-interactive",
            ])
            .current_dir(&step.dir)
            .output()
            .with_context(|| {
                format!(
                    "running `gt track {} --parent {new_parent}` in {}",
                    step.branch,
                    step.dir.display()
                )
            })?;
        if !out.status.success() {
            bail!(
                "re-parenting '{}' onto '{new_parent}' failed in {} ({}): {}\nbranch '{branch}' \
                 was not deleted; fix this by hand, then retry",
                step.branch,
                step.location.label(),
                step.dir.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        println!(
            "re-parented '{}' onto '{new_parent}' ({})",
            step.branch,
            step.location.label()
        );
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
        let repo_config = match config::repo(&config, &t.repo) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping '{}': {e:#}", t.name);
                continue;
            }
        };
        let delete_branch = match gc_verdict(&store, repo, repo_config, t) {
            Ok(GcVerdict::Skip(reason)) => {
                eprintln!("skipping '{}': {reason}", t.name);
                continue;
            }
            Err(e) => {
                eprintln!("skipping '{}': {e:#}", t.name);
                continue;
            }
            Ok(GcVerdict::Reap { delete_branch }) => delete_branch,
        };

        candidates += 1;
        let keeping = if delete_branch {
            String::new()
        } else {
            format!(
                " — keeping branch '{}': Graphite children are stacked on it",
                store::live_branch(t).unwrap_or_else(|| t.branch.clone())
            )
        };
        if opts.dry_run {
            println!("would reap '{}' ({}){keeping}", t.name, t.path.display());
            continue;
        }
        println!("reaping '{}' ({}){keeping}", t.name, t.path.display());
        if let Err(e) = rm_tree(
            root,
            config_path,
            &t.id.to_string(),
            false,
            delete_branch,
            false,
        ) {
            eprintln!("failed to reap '{}': {e:#}", t.name);
        }
    }

    if candidates == 0 {
        println!("nothing to reap");
    }
    Ok(())
}

enum GcVerdict {
    Skip(String),
    /// `delete_branch` is false when Graphite children are stacked on the
    /// branch: the worktree is still free to go, but the branch has to stay
    /// or the children lose their parent.
    Reap {
        delete_branch: bool,
    },
}

/// gc reaps the worktree, not necessarily the branch. The landed check here
/// has to stay in step with `rm_tree`'s `branch_has_unsaved_commits`: gc
/// hands every tree it picks to `rm_tree`, so a guard that is stricter than
/// this one refuses each one in turn and gc reaps nothing at all.
fn gc_verdict(
    store: &store::Store,
    repo: &Repo,
    repo_config: &config::RepoConfig,
    tree: &Tree,
) -> Result<GcVerdict> {
    if tree.spare {
        return Ok(GcVerdict::Skip("hot spare".to_string()));
    }
    if tree.state == TreeState::Provisioning {
        return Ok(GcVerdict::Skip("still provisioning".to_string()));
    }
    // A failed tree is clean and sits at trunk, so every check below would
    // wave it through and take its provisioning log with it.
    if tree.state == TreeState::Failed {
        return Ok(GcVerdict::Skip(
            "provisioning failed; read its log, then remove it with `wt rm`".to_string(),
        ));
    }
    if tree.path.exists() && git::is_dirty(&tree.path)? {
        return Ok(GcVerdict::Skip("uncommitted changes".to_string()));
    }
    // `gt create` can move a tree onto a new branch without updating the
    // registry; checking `tree.branch` instead of what's actually checked
    // out could pronounce a tree clean by looking at a branch it abandoned.
    let branch = store::live_branch(tree).unwrap_or_else(|| tree.branch.clone());
    // Most trees sit exactly at trunk, so gate the patch-id walk behind the
    // cheap count.
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
            return Ok(GcVerdict::Skip(format!(
                "{n} commit{} not yet in origin/{}",
                if n == 1 { "" } else { "s" },
                repo_config.trunk
            )));
        }
    }
    // Children only block deleting the branch, not reclaiming the worktree —
    // gc's actual job — so the tree goes and the branch stays as their parent.
    if let Some((_, children)) = stacked_children(store, &tree.repo, repo, &branch)?
        && !children.is_empty()
    {
        return Ok(GcVerdict::Reap {
            delete_branch: false,
        });
    }
    Ok(GcVerdict::Reap {
        delete_branch: true,
    })
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
                // A spare is detached by design; comparing it against a
                // recorded branch it was never meant to have would flag
                // every single one.
                Some(w) if !t.spare && w.branch.as_deref() != Some(t.branch.as_str()) => {
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
            last_fetch: None,
        };
        let repo_config = config::RepoConfig {
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            spares: 1,
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
            spare: false,
        };

        match gc_verdict(&store::Store::default(), &repo, &repo_config, &tree).unwrap() {
            GcVerdict::Skip(reason) => assert!(
                reason.contains("provisioning failed"),
                "unexpected reason: {reason}"
            ),
            GcVerdict::Reap { .. } => panic!("a failed tree must not be reaped"),
        }
    }

    #[test]
    fn gc_skips_a_spare_even_though_it_is_clean_with_no_commits() {
        let repo = Repo {
            base: PathBuf::from("/nonexistent-base"),
            last_fetch: None,
        };
        let repo_config = config::RepoConfig {
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            spares: 1,
            env: Default::default(),
            steps: Vec::new(),
        };
        // A spare with no branch and nothing dirty is exactly the state
        // that gets an ordinary tree reaped; only `tree.spare` tells gc
        // to leave it alone.
        let tree = Tree {
            id: Uuid::now_v7(),
            repo: "myrepo".into(),
            name: store::SPARE_NAME.into(),
            branch: String::new(),
            path: PathBuf::from("/nonexistent-tree"),
            created: Utc::now(),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
            parent_branch: None,
            spare: true,
        };

        match gc_verdict(&store::Store::default(), &repo, &repo_config, &tree).unwrap() {
            GcVerdict::Skip(reason) => assert_eq!(reason, "hot spare"),
            GcVerdict::Reap { .. } => panic!("a hot spare must never be reaped"),
        }
    }

    #[test]
    fn gc_reaps_a_tree_with_graphite_children_but_keeps_the_branch() {
        let dir = std::env::temp_dir().join(format!("wt-tree-gc-children-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        let git_cmd = |args: &[&str], cwd: &Path| {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        git_cmd(&["init", "-q", "-b", "master"], &base);
        git_cmd(&["config", "user.email", "t@t"], &base);
        git_cmd(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git_cmd(&["add", "-A"], &base);
        git_cmd(&["commit", "-qm", "init"], &base);
        let sha = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&base)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git_cmd(&["update-ref", "refs/remotes/origin/master", &sha], &base);
        git_cmd(&["branch", "a"], &base);
        git_cmd(&["branch", "b"], &base);
        let tree_path = dir.join("tree-a");
        git_cmd(
            &["worktree", "add", tree_path.to_str().unwrap(), "a"],
            &base,
        );
        let tree_path = fs::canonicalize(&tree_path).unwrap();

        let db = base.join(".git").join(".graphite_metadata.db");
        let sqlite = |sql: &str| {
            let out = Command::new("/usr/bin/sqlite3")
                .arg(&db)
                .arg(sql)
                .output()
                .unwrap();
            assert!(out.status.success(), "sqlite3 {sql} failed");
        };
        sqlite(
            "CREATE TABLE branch_metadata (\
             branch_name TEXT PRIMARY KEY, parent_branch_name TEXT, \
             parent_branch_revision TEXT, last_submitted_version TEXT, state TEXT, \
             children TEXT, branch_revision TEXT, validation_result TEXT, \
             parent_head_revision TEXT);",
        );
        sqlite(
            "INSERT INTO branch_metadata (branch_name, parent_branch_name, state) VALUES \
             ('master', NULL, 'TRUNK'), ('a', 'master', NULL), ('b', 'a', NULL);",
        );

        let repo = Repo {
            base: base.clone(),
            last_fetch: Some(Utc::now()),
        };
        let repo_config = config::RepoConfig {
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            spares: 1,
            env: Default::default(),
            steps: Vec::new(),
        };
        let tree = Tree {
            id: Uuid::now_v7(),
            repo: "r".into(),
            name: "tree-a".into(),
            branch: "a".into(),
            path: tree_path,
            created: Utc::now(),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
            parent_branch: None,
            spare: false,
        };
        let mut store = store::Store::default();
        store.repos.insert("r".to_string(), repo.clone());
        store.trees = vec![tree.clone()];

        match gc_verdict(&store, &repo, &repo_config, &tree).unwrap() {
            GcVerdict::Reap { delete_branch } => assert!(
                !delete_branch,
                "a branch with Graphite children stacked on it must survive gc"
            ),
            GcVerdict::Skip(reason) => {
                panic!("a tree with Graphite children must still be reaped: {reason}")
            }
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gc_reaps_a_tree_whose_commits_already_landed() {
        let dir = std::env::temp_dir().join(format!("wt-tree-gc-landed-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        let git_cmd = |args: &[&str], cwd: &Path| {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        let head_sha = |cwd: &Path| {
            String::from_utf8(
                Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(cwd)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string()
        };
        git_cmd(&["init", "-q", "-b", "master"], &base);
        git_cmd(&["config", "user.email", "t@t"], &base);
        git_cmd(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git_cmd(&["add", "-A"], &base);
        git_cmd(&["commit", "-qm", "init"], &base);
        let init_sha = head_sha(&base);
        git_cmd(
            &["update-ref", "refs/remotes/origin/master", &init_sha],
            &base,
        );
        git_cmd(&["branch", "a"], &base);
        let tree_path = dir.join("tree-a");
        git_cmd(
            &["worktree", "add", tree_path.to_str().unwrap(), "a"],
            &base,
        );
        let tree_path = fs::canonicalize(&tree_path).unwrap();

        // Same patch, different SHA — what a squash merge looks like from
        // the tree's side.
        fs::write(tree_path.join("f.txt"), "1\n").unwrap();
        git_cmd(&["add", "-A"], &tree_path);
        git_cmd(&["commit", "-qm", "change"], &tree_path);

        fs::write(base.join("f.txt"), "1\n").unwrap();
        git_cmd(&["add", "-A"], &base);
        git_cmd(&["commit", "-qm", "same change, landed on master"], &base);
        let landed_sha = head_sha(&base);
        git_cmd(
            &["update-ref", "refs/remotes/origin/master", &landed_sha],
            &base,
        );

        let repo = Repo {
            base: base.clone(),
            last_fetch: Some(Utc::now()),
        };
        let repo_config = config::RepoConfig {
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            spares: 1,
            env: Default::default(),
            steps: Vec::new(),
        };
        let tree = Tree {
            id: Uuid::now_v7(),
            repo: "r".into(),
            name: "tree-a".into(),
            branch: "a".into(),
            path: tree_path,
            created: Utc::now(),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
            parent_branch: None,
            spare: false,
        };
        let mut store = store::Store::default();
        store.repos.insert("r".to_string(), repo.clone());
        store.trees = vec![tree.clone()];

        match gc_verdict(&store, &repo, &repo_config, &tree).unwrap() {
            GcVerdict::Reap { delete_branch } => {
                assert!(delete_branch, "no Graphite children here to keep it for")
            }
            GcVerdict::Skip(reason) => panic!("unexpected skip: {reason}"),
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gc_skips_a_tree_with_an_unlanded_commit() {
        let dir = std::env::temp_dir().join(format!("wt-tree-gc-unlanded-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        let git_cmd = |args: &[&str], cwd: &Path| {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        git_cmd(&["init", "-q", "-b", "master"], &base);
        git_cmd(&["config", "user.email", "t@t"], &base);
        git_cmd(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git_cmd(&["add", "-A"], &base);
        git_cmd(&["commit", "-qm", "init"], &base);
        let init_sha = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&base)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git_cmd(
            &["update-ref", "refs/remotes/origin/master", &init_sha],
            &base,
        );
        git_cmd(&["branch", "a"], &base);
        let tree_path = dir.join("tree-a");
        git_cmd(
            &["worktree", "add", tree_path.to_str().unwrap(), "a"],
            &base,
        );
        let tree_path = fs::canonicalize(&tree_path).unwrap();

        fs::write(tree_path.join("f.txt"), "unlanded\n").unwrap();
        git_cmd(&["add", "-A"], &tree_path);
        git_cmd(&["commit", "-qm", "still open"], &tree_path);

        let repo = Repo {
            base: base.clone(),
            last_fetch: Some(Utc::now()),
        };
        let repo_config = config::RepoConfig {
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            spares: 1,
            env: Default::default(),
            steps: Vec::new(),
        };
        let tree = Tree {
            id: Uuid::now_v7(),
            repo: "r".into(),
            name: "tree-a".into(),
            branch: "a".into(),
            path: tree_path,
            created: Utc::now(),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
            parent_branch: None,
            spare: false,
        };
        let mut store = store::Store::default();
        store.repos.insert("r".to_string(), repo.clone());
        store.trees = vec![tree.clone()];

        match gc_verdict(&store, &repo, &repo_config, &tree).unwrap() {
            GcVerdict::Skip(reason) => assert!(
                reason.contains("not yet in origin/master"),
                "unexpected reason: {reason}"
            ),
            GcVerdict::Reap { .. } => panic!("an unlanded commit must not be reaped"),
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gc_reaps_a_tree_whose_path_no_longer_exists() {
        let dir = std::env::temp_dir().join(format!("wt-tree-gc-gone-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        let git_cmd = |args: &[&str], cwd: &Path| {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        git_cmd(&["init", "-q", "-b", "master"], &base);
        git_cmd(&["config", "user.email", "t@t"], &base);
        git_cmd(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git_cmd(&["add", "-A"], &base);
        git_cmd(&["commit", "-qm", "init"], &base);
        let sha = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&base)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git_cmd(&["update-ref", "refs/remotes/origin/master", &sha], &base);
        git_cmd(&["branch", "a"], &base);

        let repo = Repo {
            base: base.clone(),
            last_fetch: Some(Utc::now()),
        };
        let repo_config = config::RepoConfig {
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            spares: 1,
            env: Default::default(),
            steps: Vec::new(),
        };
        let tree = Tree {
            id: Uuid::now_v7(),
            repo: "r".into(),
            name: "tree-a".into(),
            branch: "a".into(),
            path: dir.join("tree-a-never-created"),
            created: Utc::now(),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
            parent_branch: None,
            spare: false,
        };
        let mut store = store::Store::default();
        store.repos.insert("r".to_string(), repo.clone());
        store.trees = vec![tree.clone()];

        match gc_verdict(&store, &repo, &repo_config, &tree).unwrap() {
            GcVerdict::Reap { delete_branch } => {
                assert!(delete_branch, "no Graphite children here to keep it for")
            }
            GcVerdict::Skip(reason) => panic!("unexpected skip: {reason}"),
        }

        fs::remove_dir_all(&dir).ok();
    }

    mod unsaved_commits {
        use super::*;

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

        fn head_sha(cwd: &Path) -> String {
            String::from_utf8(
                Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(cwd)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string()
        }

        fn fixture(label: &str) -> PathBuf {
            let base =
                std::env::temp_dir().join(format!("wt-tree-unsaved-{label}-{}", Uuid::now_v7()));
            fs::create_dir_all(&base).unwrap();
            git_cmd(&["init", "-q", "-b", "master"], &base);
            git_cmd(&["config", "user.email", "t@t"], &base);
            git_cmd(&["config", "user.name", "t"], &base);
            fs::write(base.join("f.txt"), "0\n").unwrap();
            git_cmd(&["add", "-A"], &base);
            git_cmd(&["commit", "-qm", "init"], &base);
            let sha = head_sha(&base);
            git_cmd(&["update-ref", "refs/remotes/origin/master", &sha], &base);
            base
        }

        /// A squash merge lands `feature`'s work on `origin/master` under a
        /// brand new SHA, leaving the branch ahead of trunk by SHA but not by
        /// patch — so nothing here is at risk of being lost.
        #[test]
        fn false_when_the_same_patch_already_landed_on_trunk_under_a_different_sha() {
            let base = fixture("landed");
            git_cmd(&["checkout", "-qb", "feature"], &base);
            fs::write(base.join("f.txt"), "1\n").unwrap();
            git_cmd(&["add", "-A"], &base);
            git_cmd(&["commit", "-qm", "change"], &base);

            git_cmd(&["checkout", "-q", "master"], &base);
            fs::write(base.join("f.txt"), "1\n").unwrap();
            git_cmd(&["add", "-A"], &base);
            git_cmd(&["commit", "-qm", "same change, landed on master"], &base);
            let landed_sha = head_sha(&base);
            git_cmd(
                &["update-ref", "refs/remotes/origin/master", &landed_sha],
                &base,
            );

            assert!(
                !branch_has_unsaved_commits(&base, "feature", "master").unwrap(),
                "a commit already landed under another SHA is not at risk"
            );

            fs::remove_dir_all(&base).ok();
        }

        #[test]
        fn true_when_unlanded_and_unpushed() {
            let base = fixture("unpushed");
            git_cmd(&["checkout", "-qb", "feature"], &base);
            fs::write(base.join("f.txt"), "unlanded\n").unwrap();
            git_cmd(&["add", "-A"], &base);
            git_cmd(&["commit", "-qm", "still open"], &base);

            assert!(
                branch_has_unsaved_commits(&base, "feature", "master").unwrap(),
                "unlanded work with no upstream to fall back on is at risk"
            );

            fs::remove_dir_all(&base).ok();
        }

        #[test]
        fn false_when_unlanded_on_trunk_but_pushed_to_its_own_upstream() {
            let base = fixture("pushed");
            // `@{upstream}` only resolves once "origin" is a configured
            // remote — the ref under refs/remotes/ is not enough on its own.
            git_cmd(&["remote", "add", "origin", "/nonexistent"], &base);
            git_cmd(&["checkout", "-qb", "feature"], &base);
            fs::write(base.join("f.txt"), "unlanded\n").unwrap();
            git_cmd(&["add", "-A"], &base);
            git_cmd(&["commit", "-qm", "still open"], &base);
            let feature_sha = head_sha(&base);
            git_cmd(
                &["update-ref", "refs/remotes/origin/feature", &feature_sha],
                &base,
            );
            git_cmd(&["config", "branch.feature.remote", "origin"], &base);
            git_cmd(
                &["config", "branch.feature.merge", "refs/heads/feature"],
                &base,
            );

            assert!(
                !branch_has_unsaved_commits(&base, "feature", "master").unwrap(),
                "work pushed to the branch's own upstream is not at risk, even if trunk \
                 hasn't merged it yet"
            );

            fs::remove_dir_all(&base).ok();
        }
    }

    mod delete_branch_guard {
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

        fn sqlite(db: &Path, sql: &str) {
            let out = Command::new("/usr/bin/sqlite3")
                .arg(db)
                .arg(sql)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "sqlite3 {sql} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        fn branch_exists(base: &Path, branch: &str) -> bool {
            Command::new("git")
                .args([
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ])
                .current_dir(base)
                .status()
                .unwrap()
                .success()
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
                    "#!/bin/sh\necho \"$* | $(pwd)\" >> \"{}\"\necho 'gt: boom' >&2\nexit 1\n",
                    log.display()
                ),
            )
            .unwrap();
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
            script
        }

        /// `master -> a -> {b, c}`: `a` is the tree being removed, `b` is
        /// held by a registered tree, and `c` is tracked by Graphite but
        /// checked out nowhere — the two holder kinds `stacked_children`
        /// must name distinctly in a refusal.
        fn fixture() -> (PathBuf, Repo, config::RepoConfig, PathBuf, PathBuf) {
            let dir = std::env::temp_dir().join(format!("wt-tree-rmguard-{}", Uuid::now_v7()));
            let base = dir.join("base");
            fs::create_dir_all(&base).unwrap();
            git_cmd(&["init", "-q", "-b", "master"], &base);
            git_cmd(&["config", "user.email", "t@t"], &base);
            git_cmd(&["config", "user.name", "t"], &base);
            fs::write(base.join("f.txt"), "0\n").unwrap();
            git_cmd(&["add", "-A"], &base);
            git_cmd(&["commit", "-qm", "init"], &base);
            let sha = String::from_utf8(
                Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(&base)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string();
            // `branch_has_unsaved_commits` diffs against `origin/<trunk>`
            // first; a fake ref stands in for a real remote so that check
            // has something to diff against.
            git_cmd(&["update-ref", "refs/remotes/origin/master", &sha], &base);
            git_cmd(&["branch", "a"], &base);
            git_cmd(&["branch", "b"], &base);
            git_cmd(&["branch", "c"], &base);

            let tree_a = dir.join("tree-a");
            git_cmd(&["worktree", "add", tree_a.to_str().unwrap(), "a"], &base);
            let tree_a = fs::canonicalize(&tree_a).unwrap();
            let tree_b = dir.join("tree-b");
            git_cmd(&["worktree", "add", tree_b.to_str().unwrap(), "b"], &base);
            let tree_b = fs::canonicalize(&tree_b).unwrap();

            let common_dir = base.join(".git");
            let db = common_dir.join(".graphite_metadata.db");
            sqlite(
                &db,
                "CREATE TABLE branch_metadata (\
                 branch_name TEXT PRIMARY KEY, parent_branch_name TEXT, \
                 parent_branch_revision TEXT, last_submitted_version TEXT, state TEXT, \
                 children TEXT, branch_revision TEXT, validation_result TEXT, \
                 parent_head_revision TEXT);",
            );
            sqlite(
                &db,
                "INSERT INTO branch_metadata (branch_name, parent_branch_name, state) VALUES \
                 ('master', NULL, 'TRUNK'), ('a', 'master', NULL), ('b', 'a', NULL), \
                 ('c', 'a', NULL);",
            );

            let repo = Repo {
                base: base.clone(),
                last_fetch: Some(Utc::now()),
            };
            let repo_config = config::RepoConfig {
                trunk: "master".into(),
                branch_prefix: "josh/".into(),
                spares: 1,
                env: Default::default(),
                steps: Vec::new(),
            };
            (dir, repo, repo_config, tree_a, tree_b)
        }

        fn sample_tree(name: &str, branch: &str, path: PathBuf) -> Tree {
            Tree {
                id: Uuid::now_v7(),
                repo: "r".into(),
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
                spare: false,
            }
        }

        fn root_with(
            dir: &Path,
            repo: &Repo,
            repo_config: &config::RepoConfig,
            trees: Vec<Tree>,
        ) -> (PathBuf, PathBuf) {
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                s.trees = trees;
                Ok(())
            })
            .unwrap();
            let config_path = dir.join("config.kdl");
            config::append_repo(&config_path, "r", repo_config).unwrap();
            (root, config_path)
        }

        #[test]
        fn refuses_and_never_invokes_gt_when_the_branch_has_children() {
            let (dir, repo, repo_config, tree_a, tree_b) = fixture();
            let tree_a_id = Uuid::now_v7();
            let mut a = sample_tree("tree-a", "a", tree_a.clone());
            a.id = tree_a_id;
            let (root, config_path) = root_with(
                &dir,
                &repo,
                &repo_config,
                vec![a, sample_tree("tree-b", "b", tree_b)],
            );

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let err = rm_tree_with(
                &root,
                &config_path,
                "tree-a",
                false,
                true,
                false,
                gt.to_str().unwrap(),
            )
            .unwrap_err();
            let msg = err.to_string();

            assert!(
                msg.contains("'b'") && msg.contains("tree-b"),
                "message: {msg}"
            );
            assert!(msg.contains("'c'"), "message: {msg}");
            assert!(msg.contains("--reparent-children"), "message: {msg}");
            assert!(!log.exists(), "gt must never run on a plain refusal");
            assert!(
                branch_exists(&repo.base, "a"),
                "branch 'a' must survive a refusal"
            );
            assert!(tree_a.exists(), "tree must survive a refusal");

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn force_bypasses_the_children_check_entirely() {
            let (dir, repo, repo_config, tree_a, tree_b) = fixture();
            let mut a = sample_tree("tree-a", "a", tree_a.clone());
            a.id = Uuid::now_v7();
            let (root, config_path) = root_with(
                &dir,
                &repo,
                &repo_config,
                vec![a, sample_tree("tree-b", "b", tree_b)],
            );

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            rm_tree_with(
                &root,
                &config_path,
                "tree-a",
                true,
                true,
                false,
                gt.to_str().unwrap(),
            )
            .unwrap();

            assert!(!log.exists(), "--force must skip the check, not reparent");
            assert!(
                !branch_exists(&repo.base, "a"),
                "--force must still delete the branch"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn reparent_children_flag_retracks_each_child_then_deletes_the_branch() {
            let (dir, repo, repo_config, tree_a, tree_b) = fixture();
            let mut a = sample_tree("tree-a", "a", tree_a.clone());
            a.id = Uuid::now_v7();
            let (root, config_path) = root_with(
                &dir,
                &repo,
                &repo_config,
                vec![a, sample_tree("tree-b", "b", tree_b.clone())],
            );

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            rm_tree_with(
                &root,
                &config_path,
                "tree-a",
                false,
                true,
                true,
                gt.to_str().unwrap(),
            )
            .unwrap();

            let log_contents = fs::read_to_string(&log).unwrap();
            assert!(
                log_contents.contains(&format!(
                    "track b --parent master --no-interactive | {}",
                    tree_b.display()
                )),
                "expected b re-parented from tree-b; log was: {log_contents}"
            );
            let base = fs::canonicalize(&repo.base).unwrap();
            assert!(
                log_contents.contains(&format!(
                    "track c --parent master --no-interactive | {}",
                    base.display()
                )),
                "expected c (held nowhere) re-parented from base; log was: {log_contents}"
            );
            assert!(
                !branch_exists(&repo.base, "a"),
                "branch 'a' must be deleted once its children are re-parented"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn a_failed_reparent_leaves_the_branch_undeleted() {
            let (dir, repo, repo_config, tree_a, tree_b) = fixture();
            let mut a = sample_tree("tree-a", "a", tree_a.clone());
            a.id = Uuid::now_v7();
            let (root, config_path) = root_with(
                &dir,
                &repo,
                &repo_config,
                vec![a, sample_tree("tree-b", "b", tree_b)],
            );

            let log = dir.join("gt-log.txt");
            let gt = fake_gt_failing(&dir, &log);
            let err = rm_tree_with(
                &root,
                &config_path,
                "tree-a",
                false,
                true,
                true,
                gt.to_str().unwrap(),
            )
            .unwrap_err();

            assert!(err.to_string().contains("not deleted"), "error: {err}");
            assert!(
                branch_exists(&repo.base, "a"),
                "a failed re-parent must not fall through to deleting the branch"
            );

            fs::remove_dir_all(&dir).ok();
        }
    }

    mod branch_drift {
        use super::*;

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

        fn branch_exists(base: &Path, branch: &str) -> bool {
            Command::new("git")
                .args([
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ])
                .current_dir(base)
                .status()
                .unwrap()
                .success()
        }

        /// A tree registered as branch `josh/started-here`, but `gt create`
        /// (simulated with a plain `checkout -b`) has since moved it onto
        /// `josh/moved-on` without the registry ever finding out — the kind
        /// of drift `Tree.branch` accumulates once Graphite moves a tree
        /// along its stack.
        #[test]
        fn delete_branch_deletes_the_live_branch_not_the_stale_recorded_one() {
            let dir = std::env::temp_dir().join(format!("wt-tree-drift-{}", Uuid::now_v7()));
            let base = dir.join("base");
            fs::create_dir_all(&base).unwrap();
            git_cmd(&["init", "-q", "-b", "master"], &base);
            git_cmd(&["config", "user.email", "t@t"], &base);
            git_cmd(&["config", "user.name", "t"], &base);
            fs::write(base.join("f.txt"), "0\n").unwrap();
            git_cmd(&["add", "-A"], &base);
            git_cmd(&["commit", "-qm", "init"], &base);
            let sha = String::from_utf8(
                Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(&base)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string();
            git_cmd(&["update-ref", "refs/remotes/origin/master", &sha], &base);

            let tree_path = dir.join("tree");
            git_cmd(
                &[
                    "worktree",
                    "add",
                    "-b",
                    "josh/started-here",
                    tree_path.to_str().unwrap(),
                ],
                &base,
            );
            let tree_path = fs::canonicalize(&tree_path).unwrap();
            git_cmd(&["checkout", "-qb", "josh/moved-on"], &tree_path);

            let repo = Repo {
                base: base.clone(),
                last_fetch: Some(Utc::now()),
            };
            let repo_config = config::RepoConfig {
                trunk: "master".into(),
                branch_prefix: "josh/".into(),
                spares: 1,
                env: Default::default(),
                steps: Vec::new(),
            };
            let tree = Tree {
                id: Uuid::now_v7(),
                repo: "r".into(),
                name: "drifted".into(),
                branch: "josh/started-here".into(),
                path: tree_path.clone(),
                created: Utc::now(),
                state: TreeState::Ready,
                step_label: None,
                step_index: None,
                step_total: None,
                log_path: None,
                provision_pid: None,
                parent_branch: None,
                spare: false,
            };
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                s.trees = vec![tree];
                Ok(())
            })
            .unwrap();
            let config_path = dir.join("config.kdl");
            config::append_repo(&config_path, "r", &repo_config).unwrap();

            rm_tree(&root, &config_path, "drifted", false, true, false).unwrap();

            assert!(
                !branch_exists(&base, "josh/moved-on"),
                "the live branch must be deleted"
            );
            assert!(
                branch_exists(&base, "josh/started-here"),
                "the stale recorded branch must survive — it was never the one in use"
            );

            fs::remove_dir_all(&dir).ok();
        }
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
                last_fetch: Some(Utc::now()),
            };
            (dir, repo)
        }

        fn sample_repo_config() -> config::RepoConfig {
            config::RepoConfig {
                trunk: "master".into(),
                branch_prefix: "josh/".into(),
                spares: 1,
                env: Default::default(),
                steps: Vec::new(),
            }
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

        fn fake_gt_untracked_parent(dir: &Path, log: &Path, parent: &str) -> PathBuf {
            let script = dir.join("gt");
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\necho \"$* | $(pwd)\" >> \"{}\"\necho 'ERROR: Cannot perform this \
                     operation on untracked branch {parent}.' >&2\nexit 1\n",
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
                spare: false,
            }
        }

        #[test]
        fn create_tree_with_onto_a_branch_name_sets_parent_and_tracks_with_graphite() {
            let (dir, repo) = fixture();
            let repo_config = sample_repo_config();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();
            let config_path = dir.join("config.kdl");
            config::append_repo(&config_path, "r", &repo_config).unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "next pr".into(),
                branch: None,
                onto: Some("stacked".into()),
                profiles: None,
            };
            let (id, tree_path, _) =
                create_tree_with(&root, &config_path, &opts, gt.to_str().unwrap()).unwrap();

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
            let repo_config = sample_repo_config();
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
            let config_path = dir.join("config.kdl");
            config::append_repo(&config_path, "r", &repo_config).unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "on top of other".into(),
                branch: None,
                onto: Some("other".into()),
                profiles: None,
            };
            let (_, tree_path, _) =
                create_tree_with(&root, &config_path, &opts, gt.to_str().unwrap()).unwrap();

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
            let repo_config = sample_repo_config();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();
            let config_path = dir.join("config.kdl");
            config::append_repo(&config_path, "r", &repo_config).unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "ignored when branch is set".into(),
                branch: Some("josh/explicit-branch".into()),
                onto: Some("stacked".into()),
                profiles: None,
            };
            let (id, tree_path, _) =
                create_tree_with(&root, &config_path, &opts, gt.to_str().unwrap()).unwrap();

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
            let repo_config = sample_repo_config();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();
            let config_path = dir.join("config.kdl");
            config::append_repo(&config_path, "r", &repo_config).unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt_failing(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "flaky".into(),
                branch: None,
                onto: Some("stacked".into()),
                profiles: None,
            };
            let (id, tree_path, _) =
                create_tree_with(&root, &config_path, &opts, gt.to_str().unwrap())
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
        fn create_tree_with_onto_keeps_the_tree_when_the_parent_is_untracked() {
            let (dir, repo) = fixture();
            let repo_config = sample_repo_config();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();
            let config_path = dir.join("config.kdl");
            config::append_repo(&config_path, "r", &repo_config).unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt_untracked_parent(&dir, &log, "stacked");
            let opts = NewOptions {
                repo: "r".into(),
                name: "on top of untracked".into(),
                branch: None,
                onto: Some("stacked".into()),
                profiles: None,
            };
            let (_, tree_path, _) =
                create_tree_with(&root, &config_path, &opts, gt.to_str().unwrap())
                    .expect("an untracked-parent failure must not fail tree creation");

            assert!(tree_path.exists());
            assert!(
                fs::read_to_string(&log)
                    .unwrap()
                    .contains("track --parent stacked"),
                "gt track must still have been attempted"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn track_failure_message_gives_the_two_step_remedy_for_an_untracked_parent() {
            let (dir, repo) = fixture();
            let repo_config = sample_repo_config();
            let holder_path = dir.join("holder-tree");
            git_cmd(
                &["worktree", "add", holder_path.to_str().unwrap(), "stacked"],
                &repo.base,
            );
            let holder_path = fs::canonicalize(&holder_path).unwrap();
            let mut store = store::Store::default();
            store.repos.insert("r".to_string(), repo.clone());
            store.trees.push(sample_tree(
                Uuid::now_v7(),
                "r",
                "holder",
                "recorded-branch",
                holder_path.clone(),
            ));
            let new_tree_path = dir.join("new-tree");
            fs::create_dir_all(&new_tree_path).unwrap();

            let stderr = "ERROR: Cannot perform this operation on untracked branch stacked.\n";
            let ctx = RepoCtx {
                name: "r",
                repo: &repo,
                config: &repo_config,
            };
            let msg =
                track_failure_message(&store, &ctx, "next pr", &new_tree_path, "stacked", stderr);

            assert!(msg.contains("tree \"holder\""), "message: {msg}");
            assert!(msg.contains("\"next pr\""), "message: {msg}");
            assert!(
                msg.contains(&format!(
                    "cd {} && gt track --parent master --no-interactive",
                    holder_path.display()
                )),
                "must name the fix-the-parent command in the tree that holds it: {msg}"
            );
            assert!(
                msg.contains(&format!(
                    "cd {} && gt track --parent stacked --no-interactive",
                    new_tree_path.display()
                )),
                "must name the retry command in the new tree: {msg}"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn track_failure_message_falls_back_to_the_generic_warning_for_any_other_failure() {
            let (dir, repo) = fixture();
            let repo_config = sample_repo_config();
            let store = store::Store::default();
            let tree_path = dir.join("tree");
            fs::create_dir_all(&tree_path).unwrap();

            let ctx = RepoCtx {
                name: "r",
                repo: &repo,
                config: &repo_config,
            };
            let msg = track_failure_message(
                &store,
                &ctx,
                "next pr",
                &tree_path,
                "stacked",
                "gt: not authenticated\n",
            );
            assert!(msg.contains("fix it by hand"), "message: {msg}");
            assert!(!msg.contains("track it first"), "message: {msg}");

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn track_failure_message_falls_back_when_the_untracked_parent_has_no_holder() {
            let (dir, repo) = fixture();
            let repo_config = sample_repo_config();
            let store = store::Store::default();
            let tree_path = dir.join("tree");
            fs::create_dir_all(&tree_path).unwrap();
            // `stacked` exists as a branch in `fixture()` but nothing has
            // it checked out — there is no directory to hand back a `cd`
            // command for, so this must degrade to the generic warning
            // rather than print one pointing nowhere.
            let ctx = RepoCtx {
                name: "r",
                repo: &repo,
                config: &repo_config,
            };
            let msg = track_failure_message(
                &store,
                &ctx,
                "next pr",
                &tree_path,
                "stacked",
                "ERROR: Cannot perform this operation on untracked branch stacked.\n",
            );
            assert!(msg.contains("fix it by hand"), "message: {msg}");

            fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn create_tree_with_no_onto_never_calls_gt() {
            let (dir, repo) = fixture();
            let repo_config = sample_repo_config();
            let root = dir.join("wtroot");
            store::with_store_lock(&root, |s| {
                s.repos.insert("r".to_string(), repo.clone());
                Ok(())
            })
            .unwrap();
            let config_path = dir.join("config.kdl");
            config::append_repo(&config_path, "r", &repo_config).unwrap();

            let log = dir.join("gt-log.txt");
            let gt = fake_gt(&dir, &log);
            let opts = NewOptions {
                repo: "r".into(),
                name: "plain".into(),
                branch: None,
                onto: None,
                profiles: None,
            };
            let (id, tree_path, _) =
                create_tree_with(&root, &config_path, &opts, gt.to_str().unwrap()).unwrap();

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
