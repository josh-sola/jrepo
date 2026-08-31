use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use uuid::Uuid;

use crate::config;
use crate::git;
use crate::proc;
use crate::provision;
use crate::store::{self, Tree, TreeState};
use crate::tree::{self, TreePlan};

/// Builds one hot spare for `repo_name` from cold and provisions it fully,
/// running to completion in the calling process — the caller is already the
/// detached background process `top_up` spawned, so there is no further
/// re-exec to do here. A no-op when the repo has opted out, or when the
/// pool is already full.
pub fn provision_spare(root: &Path, config_path: &Path, repo_name: &str) -> Result<()> {
    let store = store::load(root)?;
    let repo = store
        .repos
        .get(repo_name)
        .with_context(|| format!("unknown repo '{repo_name}'"))?
        .clone();
    let config = config::load(config_path)?;
    let repo_config = config::repo(&config, repo_name)?.clone();
    if repo_config.spares == 0 {
        return Ok(());
    }

    let id = Uuid::now_v7();
    let repo_dir = root.join(repo_name);
    let tree_path = repo_dir.join("trees").join(id.to_string());
    let log_path = tree_path.join(crate::repo::PROVISION_LOG_NAME);

    // Counting the live spares and inserting this row happen under the same
    // lock, so two `top_up` calls racing to fill the same shortfall can't
    // both see room and both insert — the second one's count already
    // includes the first's row and backs off. Registering before any git
    // work runs also matches the cold path, which registers before wiring
    // shared state for the same reason: a failure past this point has a row
    // to mark `Failed` rather than nothing to show for it at all.
    let now = Utc::now();
    let reserved = store::with_store_lock(root, |s| {
        let live = s
            .trees
            .iter()
            .filter(|t| t.repo == repo_name && t.spare)
            .filter(|t| t.state == TreeState::Ready || t.state == TreeState::Provisioning)
            .count();
        if live >= repo_config.spares as usize {
            return Ok(false);
        }
        s.trees.push(Tree {
            id,
            repo: repo_name.to_string(),
            name: store::SPARE_NAME.to_string(),
            branch: String::new(),
            path: tree_path.clone(),
            created: now,
            state: TreeState::Provisioning,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: Some(log_path.clone()),
            provision_pid: Some(std::process::id()),
            parent_branch: None,
            parent_revision: None,
            pending_restack: false,
            pr_number: None,
            spare: true,
        });
        Ok(true)
    })?;
    if !reserved {
        return Ok(());
    }

    let start_point = format!("origin/{}", repo_config.trunk);
    if let Err(e) = git::worktree_add_detached(&repo.base, &tree_path, &start_point) {
        return Err(tree::mark_failed::<()>(
            root,
            id,
            &tree_path,
            &format!("git worktree add --detach failed:\n{e:#}\n"),
            "hot spare provisioning failed",
        )
        .unwrap_err());
    }
    if let Err(e) = git::clear_worktree_hooks_path(&tree_path) {
        eprintln!("warning: could not clear inherited worktree hooksPath: {e:#}");
    }
    // Canonicalized to match the cold path, whose registered path is also
    // canonical — `wt upkeep doctor` compares registry entries against git's own
    // (canonical) worktree list, and a symlinked component in `WT_ROOT`
    // would otherwise make that comparison miss.
    let tree_path = fs::canonicalize(&tree_path)?;
    let log_path = tree_path.join(crate::repo::PROVISION_LOG_NAME);
    store::with_store_lock(root, |s| {
        if let Some(t) = s.trees.iter_mut().find(|t| t.id == id) {
            t.path = tree_path.clone();
            t.log_path = Some(log_path.clone());
        }
        Ok(())
    })?;

    if let Err(e) = tree::wire_fresh_checkout(&repo_dir, &repo.base, &tree_path) {
        return Err(tree::mark_failed::<()>(
            root,
            id,
            &tree_path,
            &format!("wiring shared state failed:\n{e:#}\n"),
            "hot spare provisioning failed",
        )
        .unwrap_err());
    }

    provision::run_steps(root, id, &tree_path, repo_name, &config, &None)
}

pub struct Claimed {
    pub id: Uuid,
    pub path: PathBuf,
    /// False when the spare's HEAD already equals the plan's start point,
    /// so the tree is ready as-is and no steps need to run.
    pub needs_steps: bool,
}

/// Turns a ready spare into `plan`'s tree, then tracks it with Graphite if
/// it has a parent.
pub fn claim(root: &Path, plan: &TreePlan) -> Result<Option<Claimed>> {
    let claimed = claim_locked(root, plan)?;

    // A spare has no branch, so it never went through the cold path's `gt
    // track`; a claimed spare with a parent needs that same call or it
    // silently drops out of the stack it was meant to join. Run after the
    // lock is released: `gt` is an external process that can be slow or
    // hang, and holding the store flock across it would block every other
    // `wt` invocation for as long as it runs.
    if let (Some(claimed), Some(parent)) = (&claimed, &plan.parent_branch) {
        let store = store::load(root)?;
        let ctx = tree::RepoCtx {
            name: &plan.repo_name,
            repo: &plan.repo,
            config: &plan.repo_config,
        };
        tree::track_with_graphite(&store, &ctx, &plan.name, &claimed.path, parent, "gt");
    }

    Ok(claimed)
}

/// This runs entirely inside one `store::with_store_lock`. That is what
/// makes the claim atomic against a concurrent `wt tree new` with no locking
/// beyond what `wt tree new` already does: two callers serialize on
/// `.data.lock`, the first to reach here takes the spare, the second finds
/// none and builds cold. It also means the common case — the spare's HEAD
/// already matches `plan.start_point` — commits to disk with no `git` call
/// beyond the branch creation below.
fn claim_locked(root: &Path, plan: &TreePlan) -> Result<Option<Claimed>> {
    store::with_store_lock(root, |s| {
        let Some(idx) = s
            .trees
            .iter()
            .position(|t| t.repo == plan.repo_name && t.spare && t.state == TreeState::Ready)
        else {
            return Ok(None);
        };

        let spare_path = s.trees[idx].path.clone();
        let spare_id = s.trees[idx].id;
        // A spare is an optimization; nothing about it may ever be
        // load-bearing for `wt tree new`. A directory deleted by hand or a
        // corrupt git dir degrades to "no usable spare" here, the same as
        // a failed `switch` below, rather than failing the command outright.
        let Ok(head) = git::rev_parse(&spare_path, "HEAD") else {
            return Ok(None);
        };
        let needs_steps = head != plan.start_point;

        let saved = s.trees[idx].clone();
        {
            let t = &mut s.trees[idx];
            t.spare = false;
            t.name = plan.name.clone();
            t.branch = plan.branch.clone();
            t.parent_branch = plan.parent_branch.clone();
            t.parent_revision = plan.parent_revision.clone();
            t.created = Utc::now();
            t.state = if needs_steps {
                TreeState::Provisioning
            } else {
                TreeState::Ready
            };
            t.step_label = None;
            t.step_index = None;
            t.step_total = None;
        }

        // A dirty working tree left by a half-finished provisioning step
        // fails here; that is expected and survivable, not a bug to work
        // around — the row goes back exactly as it was and the caller
        // builds from cold instead.
        if let Err(e) = git::switch_new_branch(&spare_path, &plan.branch, &plan.start_point) {
            eprintln!(
                "warning: claiming a hot spare failed, building the tree from cold instead: {e:#}"
            );
            s.trees[idx] = saved;
            return Ok(None);
        }

        // The claimed tree's log should hold its own output, not whatever
        // the spare wrote while it sat idle.
        let log_path = spare_path.join(crate::repo::PROVISION_LOG_NAME);
        fs::write(&log_path, "").ok();

        Ok(Some(Claimed {
            id: spare_id,
            path: spare_path,
            needs_steps,
        }))
    })
}

/// Spawns detached provisioning for any shortfall against `repo.spares`.
/// Reaps a failed spare, or one stuck `provisioning` behind a dead pid,
/// before counting — otherwise a spare that failed once would sit in the
/// registry forever, permanently short-circuiting every future top-up.
pub fn top_up(root: &Path, config_path: &Path, repo_filter: Option<&str>) -> Result<()> {
    let store = store::load(root)?;
    let config = config::load(config_path)?;
    let repo_names: Vec<String> = match repo_filter {
        Some(r) => vec![r.to_string()],
        None => store.repos.keys().cloned().collect(),
    };

    for repo_name in repo_names {
        // A repo registered on this machine but missing from config is a
        // real error elsewhere, but a background top-up has nothing useful
        // to do about it beyond skipping this one repo.
        let Ok(repo_config) = config::repo(&config, &repo_name) else {
            continue;
        };
        if repo_config.spares == 0 {
            continue;
        }

        reap_dead_spares(root, &repo_name)?;

        let live = store::load(root)?
            .trees
            .iter()
            .filter(|t| t.repo == repo_name && t.spare)
            .filter(|t| t.state == TreeState::Ready || t.state == TreeState::Provisioning)
            .count();

        for _ in live..repo_config.spares as usize {
            proc::spawn_detached(root, config_path, &["__spare", "new", &repo_name])?;
        }
    }
    Ok(())
}

/// Removes every spare for `repo_name` that is `Failed`, or `Provisioning`
/// behind a pid that's no longer alive — the same staleness check `wt
/// status` uses to flag a wedged ordinary tree.
fn reap_dead_spares(root: &Path, repo_name: &str) -> Result<()> {
    let dead: Vec<Uuid> = store::load(root)?
        .trees
        .iter()
        .filter(|t| t.repo == repo_name && t.spare)
        .filter(|t| t.state == TreeState::Failed || proc::provisioning_is_stale(t))
        .map(|t| t.id)
        .collect();
    for id in dead {
        remove_spare(root, id)?;
    }
    Ok(())
}

/// Removes a spare directly by uuid. Deliberately not `tree::rm_tree`:
/// `store::resolve` filters spares out at every tier, and an unclaimed
/// spare has no branch and no Graphite history for that function's guards
/// to check in the first place. Only `top_up`'s own reaping and `wt repo spare
/// drop` ever call this, always with an id already known to be a spare's.
fn remove_spare(root: &Path, id: Uuid) -> Result<()> {
    let store = store::load(root)?;
    let Some(t) = store.trees.iter().find(|t| t.id == id && t.spare) else {
        return Ok(());
    };
    let tree_path = t.path.clone();
    let repo_base = store.repos.get(&t.repo).map(|r| r.base.clone());

    if tree_path.exists()
        && let Err(e) = tree::remove_tree_dir(&tree_path)
    {
        eprintln!("warning: {e:#} (removing hot spare {id})");
    }
    if let Some(base) = repo_base
        && let Err(e) = git::worktree_prune(&base)
    {
        eprintln!("warning: git worktree prune failed: {e}");
    }
    store::with_store_lock(root, |s| {
        s.trees.retain(|t| t.id != id);
        Ok(())
    })?;
    Ok(())
}

/// Fast-forwards each idle spare in `repo_filter` (or every repo) to
/// `origin/<trunk>` and re-provisions it when trunk has moved on. A spare
/// still `provisioning` is left alone — that's what keeps two overlapping
/// `wt repo sync` ticks out of each other's way.
pub fn refresh(root: &Path, config_path: &Path, repo_filter: Option<&str>) -> Result<()> {
    let store = store::load(root)?;
    let config = config::load(config_path)?;
    for t in &store.trees {
        if !t.spare || t.state != TreeState::Ready {
            continue;
        }
        if let Some(r) = repo_filter
            && t.repo != *r
        {
            continue;
        }
        let Some(repo) = store.repos.get(&t.repo) else {
            continue;
        };
        let Ok(repo_config) = config::repo(&config, &t.repo) else {
            continue;
        };
        let trunk_ref = format!("origin/{}", repo_config.trunk);
        let Ok(trunk_head) = git::rev_parse(&repo.base, &trunk_ref) else {
            continue;
        };
        let Ok(spare_head) = git::rev_parse(&t.path, "HEAD") else {
            continue;
        };
        if spare_head == trunk_head {
            continue;
        }

        // Flipping the row here, under the lock, before spawning is what
        // keeps two overlapping callers (a `wt repo spare refresh` racing a `wt
        // repo sync` tick) from both reading `Ready` and both starting a
        // `checkout --detach` plus a dependency install in the same
        // directory. Only the caller that wins the flip spawns; the loser
        // sees `Ready` gone and moves on to the next spare.
        let id = t.id;
        let claimed_it = store::with_store_lock(root, |s| {
            let Some(row) = s.trees.iter_mut().find(|x| x.id == id) else {
                return Ok(false);
            };
            if row.state != TreeState::Ready {
                return Ok(false);
            }
            row.state = TreeState::Provisioning;
            // A placeholder, not the eventual worker's pid: this process
            // is `wt repo sync` or `wt repo spare refresh` and is about to exit. It
            // exists only so the row is never `provisioning` with no pid
            // at all, which `proc::provisioning_is_stale` would never flag
            // — `run_refresh` overwrites it with its own pid immediately.
            row.provision_pid = Some(std::process::id());
            Ok(true)
        })?;
        if claimed_it {
            proc::spawn_detached(root, config_path, &["__spare", "refresh", &id.to_string()])?;
        }
    }
    Ok(())
}

/// The work behind `wt __spare refresh <id>`: fast-forward and re-provision
/// as one unit, sitting in `provisioning` for the whole of it. That's the
/// invariant a claim relies on — a `ready` spare has finished every step at
/// its current HEAD — so nothing may move a spare's HEAD without redoing
/// its steps in the same operation.
pub fn run_refresh(root: &Path, config_path: &Path, id: Uuid) -> Result<()> {
    let store = store::load(root)?;
    let tree = store
        .trees
        .iter()
        .find(|t| t.id == id && t.spare)
        .with_context(|| format!("spare {id} not found in registry"))?
        .clone();
    let repo = store
        .repos
        .get(&tree.repo)
        .with_context(|| format!("spare {id} references unknown repo '{}'", tree.repo))?
        .clone();
    let config = config::load(config_path)?;
    let repo_config = config::repo(&config, &tree.repo)?.clone();

    store::with_store_lock(root, |s| {
        if let Some(t) = s.trees.iter_mut().find(|t| t.id == id) {
            t.state = TreeState::Provisioning;
            t.provision_pid = Some(std::process::id());
        }
        Ok(())
    })?;

    let trunk_ref = format!("origin/{}", repo_config.trunk);
    if let Err(e) = git::checkout_detached(&tree.path, &trunk_ref) {
        return Err(tree::mark_failed::<()>(
            root,
            id,
            &tree.path,
            &format!("checking out {trunk_ref} failed:\n{e:#}\n"),
            "hot spare refresh failed",
        )
        .unwrap_err());
    }

    let repo_dir = root.join(&tree.repo);
    if let Err(e) = tree::wire_fresh_checkout(&repo_dir, &repo.base, &tree.path) {
        return Err(tree::mark_failed::<()>(
            root,
            id,
            &tree.path,
            &format!("wiring shared state failed:\n{e:#}\n"),
            "hot spare refresh failed",
        )
        .unwrap_err());
    }

    provision::run_steps(root, id, &tree.path, &tree.repo, &config, &None)
}

/// Removes every one of `repo_name`'s spares — ordinarily just one, but
/// nothing here assumes that. Also sets `spares` to 0 in config, so a
/// `wt repo sync` tick five minutes later can't quietly rebuild the one this
/// just removed.
pub fn drop_spare(root: &Path, config_path: &Path, repo_name: &str) -> Result<()> {
    let store = store::load(root)?;
    if !store.repos.contains_key(repo_name) {
        anyhow::bail!(
            "unknown repo '{repo_name}'. Known repos: {}",
            if store.repos.is_empty() {
                "(none registered)".to_string()
            } else {
                store.repos.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        );
    }
    let spare_ids: Vec<Uuid> = store
        .trees
        .iter()
        .filter(|t| t.repo == repo_name && t.spare)
        .map(|t| t.id)
        .collect();
    for id in spare_ids {
        remove_spare(root, id)?;
    }
    config::set_repo_spares(config_path, repo_name, 0)?;
    Ok(())
}
