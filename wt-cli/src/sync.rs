use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::config;
use crate::git;
use crate::restack::{self, Step};
use crate::stack::{self, Stacks};
use crate::store::{self, Repo, Store};

/// Fetches every registered repo (or just `repo_filter`) and fast-forwards
/// its trunk when safe. With `stack`, also walks every Graphite stack in the
/// repo that spans more than one worktree, restacking it bottom-up — the
/// same walk `wt restack` runs, just over every such stack instead of one.
/// Never destructive: a repo that fails to sync is reported and skipped
/// rather than aborting the rest.
pub fn sync(
    root: &Path,
    config_path: &Path,
    repo_filter: Option<String>,
    stack: bool,
) -> Result<()> {
    let store = store::load(root)?;
    let config = config::load(config_path)?;
    let repos: Vec<(String, Repo)> = match repo_filter {
        Some(name) => {
            let repo = store
                .repos
                .get(&name)
                .cloned()
                .with_context(|| format!("unknown repo '{name}'"))?;
            vec![(name, repo)]
        }
        None => store
            .repos
            .iter()
            .map(|(n, r)| (n.clone(), r.clone()))
            .collect(),
    };

    if repos.is_empty() {
        println!("no repos registered");
        return Ok(());
    }

    let mut had_failure = false;
    for (name, repo) in &repos {
        let repo_config = match config::repo(&config, name) {
            Ok(c) => c,
            Err(e) => {
                had_failure = true;
                println!("{name}: {e:#}");
                continue;
            }
        };
        match sync_one(root, name, repo, repo_config) {
            Ok(line) => {
                println!("{name}: {line}");
                if stack && let Err(e) = sync_stack(root, name, repo) {
                    had_failure = true;
                    println!("{name}: {e:#}");
                }
                // Both spawn detached and never fail the sync run: this
                // runs on a 5-minute timer and must not block on a
                // `pnpm install`, and one repo's spare trouble is no
                // reason to fail the rest of the sync.
                if let Err(e) = crate::spare::refresh(root, config_path, Some(name)) {
                    println!("{name}: hot spare refresh failed: {e:#}");
                }
                if let Err(e) = crate::spare::top_up(root, config_path, Some(name)) {
                    println!("{name}: hot spare top-up failed: {e:#}");
                }
            }
            Err(e) => {
                had_failure = true;
                println!("{name}: {e:#}");
            }
        }
    }

    if had_failure {
        bail!("one or more repos failed to sync");
    }
    Ok(())
}

/// Drains one tree's own restack debt: `wt restack`/`wt repo sync
/// --stack` walk a whole stack and mark what they can't reach; this is the
/// other half, run from (or naming) the one tree whose turn has come. If
/// its branch doesn't need a restack — by the stored flag or a fresh
/// check — this says so and does nothing else.
pub fn sync_tree(root: &Path, selector: Option<String>) -> Result<()> {
    sync_tree_with(root, selector, "gt")
}

fn sync_tree_with(root: &Path, selector: Option<String>, gt_bin: &str) -> Result<()> {
    let store = store::load(root)?;
    let tree = resolve_tree(&store, selector.as_deref())?;
    let repo = store
        .repos
        .get(&tree.repo)
        .with_context(|| format!("repo '{}' is not registered", tree.repo))?;
    let branch = store::live_branch(tree).unwrap_or_else(|| tree.branch.clone());

    let entry = stack::load(&tree.repo, repo, &store)?.and_then(|s| s.get(&branch).cloned());
    let Some(entry) = entry else {
        println!("'{branch}' doesn't need a restack");
        return Ok(());
    };
    if !entry.shows_needs_restack() {
        println!("'{branch}' doesn't need a restack");
        return Ok(());
    }

    let step = restack::step_for(&entry, &store, repo);
    let reasons = restack::readiness(&step);
    if !reasons.is_empty() {
        bail!("can't restack '{branch}': {}", reasons.join(", "));
    }

    match restack::restack_one_with(root, &tree.repo, &step, gt_bin)? {
        restack::RestackAttempt::Restacked => println!("restacked '{branch}'"),
        restack::RestackAttempt::Blocked(reason) => bail!("restack stopped: {reason}"),
    }

    let children = mark_children_pending(root, &tree.repo, &branch)?;
    if children.is_empty() {
        println!("nothing else is stacked on '{branch}'");
    } else {
        println!(
            "now pending a restack of their own: {}",
            children.join(", ")
        );
    }
    Ok(())
}

/// The tree a bare `wt sync` (or `wt submit`) acts on: the one named by
/// `selector`, or the one containing the current directory when it's
/// omitted.
pub(crate) fn resolve_tree<'a>(
    store: &'a Store,
    selector: Option<&str>,
) -> Result<&'a store::Tree> {
    if let Some(sel) = selector {
        return store::resolve(&store.trees, sel);
    }
    let cwd = std::env::current_dir().context("reading current directory")?;
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    store
        .trees
        .iter()
        .filter(|t| !t.spare && cwd.starts_with(&t.path))
        .max_by_key(|t| t.path.components().count())
        .with_context(|| {
            "the current directory isn't inside a tree; pass a tree name, or run this from \
             inside one"
        })
}

/// Every tree whose parent branch is `branch`: their restack was only
/// correct against `branch`'s old position, which just moved.
fn mark_children_pending(root: &Path, repo_name: &str, branch: &str) -> Result<Vec<String>> {
    store::with_store_lock(root, |s| {
        let mut marked = Vec::new();
        for t in &mut s.trees {
            if t.repo == repo_name && t.parent_branch.as_deref() == Some(branch) {
                t.pending_restack = true;
                marked.push(t.branch.clone());
            }
        }
        Ok(marked)
    })
}

/// Restacks every stack in `repo` that has branches held by more than one
/// worktree — a single-tree stack has no cross-worktree problem for this
/// walk to solve. A tree that isn't ready only skips its own branch and
/// whatever sits on top of it, marked `pending_restack` for a later `wt
/// sync`; a real `gt` conflict still stops the whole sync, same as before,
/// leaving later stacks in the repo unwalked.
fn sync_stack(root: &Path, name: &str, repo: &Repo) -> Result<()> {
    let store = store::load(root)?;
    let Some(stacks) = stack::load(name, repo, &store)? else {
        println!("{name}: no trees yet, skipping the restack walk");
        return Ok(());
    };

    let to_walk = stacks_to_walk(&stacks, &store, repo);
    let mut restacked = 0;
    let mut pending = 0;
    for (r, steps) in &to_walk {
        let outcome = restack::walk(root, name, steps)?;
        restacked += outcome.restacked.len();
        pending += outcome.pending.len();
        for line in outcome.describe() {
            println!("{name}: '{r}' stack: {line}");
        }
    }

    let walked = to_walk.len();
    println!(
        "{name}: {}",
        if walked == 0 {
            "no multi-tree stacks to restack".to_string()
        } else {
            format!(
                "walked {walked} multi-tree stack{} — {restacked} branch{} restacked, \
                 {pending} pending",
                if walked == 1 { "" } else { "s" },
                if restacked == 1 { "" } else { "es" },
            )
        }
    );
    Ok(())
}

/// Every stack in `stacks`, rooted at its trunk or an untracked orphan, that
/// would touch more than one directory — a stack held entirely by one
/// worktree (or by none) has no cross-worktree problem for this walk to
/// solve, so it's left for a plain `gt restack` inside that one tree.
fn stacks_to_walk(stacks: &Stacks, store: &Store, repo: &Repo) -> Vec<(String, Vec<Step>)> {
    let mut roots = stacks.graph.roots();
    roots.sort();

    roots
        .into_iter()
        .filter_map(|r| {
            let branches = stacks.graph.upstack(&r);
            let steps = restack::plan(stacks, &branches, store, repo);
            if distinct_dirs(&steps) > 1 {
                Some((r, steps))
            } else {
                None
            }
        })
        .collect()
}

fn distinct_dirs(steps: &[Step]) -> usize {
    steps.iter().map(|s| &s.dir).collect::<HashSet<_>>().len()
}

/// The base's state, in the order it must be checked: a base with its own
/// uncommitted changes always blocks, before anything submodule-related is
/// even considered.
#[derive(Debug)]
enum BaseState {
    Clean,
    /// Stale or uninitialized submodule paths, each confirmed to hold
    /// nothing uncommitted of its own — safe to move.
    Repairable(Vec<String>),
    /// Human-readable reason naming what is in the way.
    Blocked(String),
}

fn repaired_note(paths: &[String]) -> String {
    format!(
        "repaired {} stale submodule pointer{}",
        paths.len(),
        if paths.len() == 1 { "" } else { "s" }
    )
}

/// A stale gitlink, a submodule with uncommitted work of its own, and the
/// base's own uncommitted changes all look identical to a plain `git status
/// --porcelain`. Only the first is safe to repair automatically; the other
/// two must still block a fast-forward exactly as an unfiltered dirty check
/// always has.
fn classify_base(base: &Path) -> Result<BaseState> {
    let own_changes = git::status_porcelain_filtered(base, git::SubmoduleFilter::All)?;
    if !own_changes.is_empty() {
        return Ok(BaseState::Blocked(format!(
            "dirty, refusing to fast-forward ({}): {}",
            own_changes.len(),
            own_changes.join("; ")
        )));
    }

    let submodules = git::submodule_status(base)?;
    if let Some(conflicted) = submodules
        .iter()
        .find(|s| matches!(s.state, git::SubmoduleState::Conflicted))
    {
        return Ok(BaseState::Blocked(format!(
            "submodule '{}' has unresolved merge conflicts, refusing to fast-forward",
            conflicted.path
        )));
    }

    let candidates: Vec<&git::SubmoduleEntry> = submodules
        .iter()
        .filter(|s| {
            matches!(
                s.state,
                git::SubmoduleState::StalePointer | git::SubmoduleState::Uninitialized
            )
        })
        .collect();

    if candidates.is_empty() {
        let plain = git::status_porcelain(base)?;
        if !plain.is_empty() {
            return Ok(BaseState::Blocked(format!(
                "dirty, refusing to fast-forward ({}): {}",
                plain.len(),
                plain.join("; ")
            )));
        }
        return Ok(BaseState::Clean);
    }

    for entry in &candidates {
        if matches!(entry.state, git::SubmoduleState::Uninitialized) {
            continue;
        }
        let dirty = git::status_porcelain(&base.join(&entry.path))?;
        if !dirty.is_empty() {
            return Ok(BaseState::Blocked(format!(
                "submodule '{}' has uncommitted changes, refusing to move it ({}): {}",
                entry.path,
                dirty.len(),
                dirty.join("; ")
            )));
        }
    }

    Ok(BaseState::Repairable(
        candidates.into_iter().map(|e| e.path.clone()).collect(),
    ))
}

fn sync_one(
    root: &Path,
    name: &str,
    repo: &Repo,
    repo_config: &config::RepoConfig,
) -> Result<String> {
    let trunk_ref = format!("origin/{}", repo_config.trunk);
    let before = git::rev_parse(&repo.base, &trunk_ref).ok();

    git::fetch_prune(&repo.base)?;
    store::with_store_lock(root, |s| {
        if let Some(r) = s.repos.get_mut(name) {
            r.last_fetch = Some(Utc::now());
        }
        Ok(())
    })?;

    let after = git::rev_parse(&repo.base, &trunk_ref)
        .with_context(|| format!("resolving {trunk_ref} after fetch"))?;
    let fetch_desc = if before.as_deref() == Some(after.as_str()) {
        "up to date"
    } else {
        "fetched new commits"
    };

    let repair_note = match classify_base(&repo.base)? {
        BaseState::Blocked(reason) => bail!(reason),
        BaseState::Repairable(paths) => {
            git::submodule_update_recursive(&repo.base)
                .with_context(|| format!("repairing {} stale submodule pointer(s)", paths.len()))?;
            Some(repaired_note(&paths))
        }
        BaseState::Clean => None,
    };
    let prefixed = |rest: String| match &repair_note {
        Some(note) => format!("{note}; {rest}"),
        None => rest,
    };

    let branch = git::current_branch(&repo.base)?;
    if branch != repo_config.trunk {
        return Ok(prefixed(format!(
            "{fetch_desc}; on branch '{branch}', not '{}' — skipping fast-forward",
            repo_config.trunk
        )));
    }

    let head = git::rev_parse(&repo.base, "HEAD").context("resolving HEAD")?;
    if head == after {
        return Ok(prefixed(format!("{fetch_desc}; trunk unchanged")));
    }

    git::merge_ff_only(&repo.base, &trunk_ref)?;
    let mut line = prefixed(format!(
        "{fetch_desc}; fast-forwarded {} to {}",
        repo_config.trunk,
        &after[..after.len().min(7)]
    ));

    // Non-fatal: the fast-forward already succeeded, so a submodule that
    // can't be repaired afterward is a warning on the status line, not a
    // failed sync.
    match classify_base(&repo.base) {
        Ok(BaseState::Repairable(paths)) => match git::submodule_update_recursive(&repo.base) {
            Ok(()) => line.push_str(&format!("; {} after fast-forward", repaired_note(&paths))),
            Err(e) => line.push_str(&format!(
                "; warning: fast-forward left {} submodule pointer(s) stale and the repair \
                 failed: {e:#}",
                paths.len()
            )),
        },
        Ok(BaseState::Blocked(reason)) => {
            line.push_str(&format!(
                "; warning: fast-forward left a submodule that needs attention: {reason}"
            ));
        }
        Ok(BaseState::Clean) => {}
        Err(e) => line.push_str(&format!(
            "; warning: could not verify submodule state after fast-forward: {e:#}"
        )),
    }

    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use chrono::Utc;
    use uuid::Uuid;

    use crate::store::{Tree, TreeState};

    fn git(args: &[&str], cwd: &Path) {
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

    fn sample_tree(name: &str, branch: &str, parent_branch: Option<&str>, path: PathBuf) -> Tree {
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
            parent_branch: parent_branch.map(str::to_string),
            parent_revision: None,
            pending_restack: false,
            pr_number: None,
            spare: false,
        }
    }

    /// Two independent stacks off the same repo: `s1a` (parent trunk), held
    /// by a single tree, and `s2a -> s2b`, split across two — `s2a`'s parent
    /// names a branch no tree ever backed, so `s2a` is the graph's root for
    /// that stack. Only the second stack has a cross-worktree problem for
    /// `--stack` to solve.
    fn fixture() -> (PathBuf, Repo, Store, Stacks) {
        let dir = std::env::temp_dir().join(format!("wt-sync-stack-test-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        git(&["init", "-q", "-b", "master"], &base);
        git(&["config", "user.email", "t@t"], &base);
        git(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);
        for b in ["s1a", "s2a", "s2b"] {
            git(&["branch", b], &base);
        }

        let tree_s1a = dir.join("tree-s1a");
        git(
            &["worktree", "add", tree_s1a.to_str().unwrap(), "s1a"],
            &base,
        );
        let tree_s2a = dir.join("tree-s2a");
        git(
            &["worktree", "add", tree_s2a.to_str().unwrap(), "s2a"],
            &base,
        );
        let tree_s2b = dir.join("tree-s2b");
        git(
            &["worktree", "add", tree_s2b.to_str().unwrap(), "s2b"],
            &base,
        );

        let repo = Repo {
            base: base.clone(),
            last_fetch: None,
        };

        let mut store = Store::default();
        store.repos.insert("r".to_string(), repo.clone());
        store.trees = vec![
            sample_tree(
                "tree-s1a",
                "s1a",
                Some("master"),
                fs::canonicalize(&tree_s1a).unwrap(),
            ),
            sample_tree(
                "tree-s2a",
                "s2a",
                Some("root2"),
                fs::canonicalize(&tree_s2a).unwrap(),
            ),
            sample_tree(
                "tree-s2b",
                "s2b",
                Some("s2a"),
                fs::canonicalize(&tree_s2b).unwrap(),
            ),
        ];

        let stacks = stack::load("r", &repo, &store).unwrap().unwrap();
        (dir, repo, store, stacks)
    }

    #[test]
    fn stacks_to_walk_skips_a_single_tree_stack_and_keeps_a_multi_tree_one() {
        let (dir, repo, store, stacks) = fixture();

        let walked = stacks_to_walk(&stacks, &store, &repo);
        assert_eq!(walked.len(), 1, "only s2a's stack spans more than one tree");
        let (root, steps) = &walked[0];
        // `root2` names no tree of its own, so `s2a` — the lowest branch a
        // tree actually backs — is the graph's root for this stack.
        assert_eq!(root, "s2a");
        let branches: Vec<&str> = steps.iter().map(|s| s.branch.as_str()).collect();
        assert_eq!(branches, vec!["s2a", "s2b"]);

        fs::remove_dir_all(&dir).ok();
    }

    /// A base repo with `sub` added as a submodule at HEAD, both on tracked
    /// commits. Local-path submodules need `protocol.file.allow=always` —
    /// git refuses `file://` submodules otherwise.
    fn submodule_fixture() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wt-sync-submodule-test-{}", Uuid::now_v7()));
        let sub = dir.join("sub");
        let base = dir.join("base");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(&base).unwrap();
        for path in [&sub, &base] {
            git(&["init", "-q", "-b", "main"], path);
            git(&["config", "user.email", "t@t"], path);
            git(&["config", "user.name", "t"], path);
        }

        fs::write(sub.join("f.txt"), "one\n").unwrap();
        git(&["add", "-A"], &sub);
        git(&["commit", "-qm", "init"], &sub);

        fs::write(base.join("g.txt"), "x\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);
        git(
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                sub.to_str().unwrap(),
                "sub",
            ],
            &base,
        );
        git(&["commit", "-qm", "add submodule"], &base);
        (base, sub)
    }

    fn cleanup_submodule_fixture(base: &Path) {
        fs::remove_dir_all(base.parent().unwrap()).ok();
    }

    #[test]
    fn classify_base_is_clean_when_nothing_is_dirty() {
        let (base, _sub) = submodule_fixture();

        assert!(matches!(classify_base(&base).unwrap(), BaseState::Clean));

        cleanup_submodule_fixture(&base);
    }

    #[test]
    fn classify_base_is_repairable_for_a_stale_pointer_with_a_clean_submodule() {
        let (base, _sub) = submodule_fixture();
        fs::write(base.join("sub").join("f.txt"), "two\n").unwrap();
        git(&["commit", "-qam", "advance"], &base.join("sub"));

        match classify_base(&base).unwrap() {
            BaseState::Repairable(paths) => assert_eq!(paths, vec!["sub".to_string()]),
            other => panic!("expected Repairable, got {other:?}"),
        }

        cleanup_submodule_fixture(&base);
    }

    #[test]
    fn classify_base_blocks_on_uncommitted_work_inside_a_submodule() {
        let (base, _sub) = submodule_fixture();
        fs::write(base.join("sub").join("f.txt"), "edited\n").unwrap();

        match classify_base(&base).unwrap() {
            BaseState::Blocked(reason) => assert!(
                reason.contains("sub"),
                "expected the submodule to be named: {reason}"
            ),
            other => panic!("expected Blocked, got {other:?}"),
        }

        cleanup_submodule_fixture(&base);
    }

    #[test]
    fn classify_base_blocks_on_changes_in_the_base_itself() {
        let (base, _sub) = submodule_fixture();
        fs::write(base.join("g.txt"), "changed\n").unwrap();

        match classify_base(&base).unwrap() {
            BaseState::Blocked(reason) => assert!(
                reason.contains("dirty, refusing to fast-forward"),
                "unexpected reason: {reason}"
            ),
            other => panic!("expected Blocked, got {other:?}"),
        }

        cleanup_submodule_fixture(&base);
    }

    #[test]
    fn classify_base_blocks_a_stale_pointer_that_also_has_uncommitted_work() {
        let (base, _sub) = submodule_fixture();
        fs::write(base.join("sub").join("f.txt"), "two\n").unwrap();
        git(&["commit", "-qam", "advance"], &base.join("sub"));
        fs::write(base.join("sub").join("f.txt"), "three\n").unwrap();

        match classify_base(&base).unwrap() {
            BaseState::Blocked(reason) => assert!(
                reason.contains("sub"),
                "expected the submodule to be named: {reason}"
            ),
            other => panic!(
                "a submodule with both a stale pointer and its own uncommitted work must \
                 block, got {other:?}"
            ),
        }

        cleanup_submodule_fixture(&base);
    }

    fn fake_gt_always_succeeds(dir: &Path) -> PathBuf {
        let script = dir.join("gt");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    /// `b` is pending a restack (flag set by hand, standing in for a walk
    /// that marked it earlier) and `c`'s parent is `b`. Draining `b` should
    /// restack it, clear its own flag, and mark `c` pending in turn — its
    /// old restack is only correct against where `b` used to be.
    #[test]
    fn sync_tree_drains_a_pending_branch_and_marks_its_children_pending() {
        let dir = std::env::temp_dir().join(format!("wt-sync-tree-test-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        git(&["init", "-q", "-b", "master"], &base);
        git(&["config", "user.email", "t@t"], &base);
        git(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);

        let tree_b_path = dir.join("tree-b");
        git(
            &["worktree", "add", tree_b_path.to_str().unwrap(), "-b", "b"],
            &base,
        );

        let root = dir.join("wtroot");
        let repo = Repo {
            base: base.clone(),
            last_fetch: None,
        };
        store::with_store_lock(&root, |s| {
            s.repos.insert("r".to_string(), repo.clone());
            let mut tree_b = sample_tree("tree-b", "b", Some("a"), tree_b_path.clone());
            tree_b.pending_restack = true;
            let tree_c = sample_tree("tree-c", "c", Some("b"), dir.join("tree-c-no-worktree"));
            s.trees = vec![tree_b, tree_c];
            Ok(())
        })
        .unwrap();

        let gt = fake_gt_always_succeeds(&dir);
        sync_tree_with(&root, Some("tree-b".to_string()), gt.to_str().unwrap()).unwrap();

        let store = store::load(&root).unwrap();
        let by_branch = |b: &str| store.trees.iter().find(|t| t.branch == b).unwrap();
        assert!(!by_branch("b").pending_restack, "b was just drained");
        assert!(by_branch("c").pending_restack, "b just moved under c");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sync_tree_says_so_plainly_when_nothing_is_needed() {
        let dir = std::env::temp_dir().join(format!("wt-sync-tree-noop-test-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        git(&["init", "-q", "-b", "master"], &base);
        git(&["config", "user.email", "t@t"], &base);
        git(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);

        let tree_a_path = dir.join("tree-a");
        git(
            &["worktree", "add", tree_a_path.to_str().unwrap(), "-b", "a"],
            &base,
        );

        let root = dir.join("wtroot");
        let repo = Repo {
            base: base.clone(),
            last_fetch: None,
        };
        store::with_store_lock(&root, |s| {
            s.repos.insert("r".to_string(), repo.clone());
            s.trees = vec![sample_tree("tree-a", "a", Some("master"), tree_a_path)];
            Ok(())
        })
        .unwrap();

        // A never-invoked `gt` proves the no-restack path never shells out.
        let gt = dir.join("gt-never-run");
        sync_tree_with(&root, Some("tree-a".to_string()), gt.to_str().unwrap()).unwrap();

        let store = store::load(&root).unwrap();
        assert!(!store.trees[0].pending_restack);

        fs::remove_dir_all(&dir).ok();
    }
}
