//! Drives `gt restack --branch <b> --only` across every worktree that holds
//! a branch in a Graphite stack. `gt` refuses to touch a branch checked out
//! in another worktree, so `wt` — the only thing that can see every
//! worktree at once — has to run each branch's restack from wherever it
//! actually lives, bottom-up.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::git;
use crate::stack::{self, Stacks};
use crate::store::{self, Repo, Store, TreeState};

/// Where a step's `gt restack` runs, kept apart from `Step.dir` so preflight
/// and printing can name the tree without re-deriving it from a bare path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Tree { name: String, provisioning: bool },
    Unregistered,
    Base,
}

impl Location {
    pub fn label(&self) -> String {
        match self {
            Location::Tree { name, .. } => name.clone(),
            Location::Unregistered => "[unregistered]".to_string(),
            Location::Base => "[base]".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    pub branch: String,
    pub parent: Option<String>,
    pub dir: PathBuf,
    pub location: Location,
}

/// Every branch in `branches` that is still live, bottom-up, one step each.
/// Not just the branches currently flagged `needs_restack` — restacking a
/// branch can make its child need one too, so a set computed up front would
/// already be stale by the time the walk gets there. `gt restack --only` is
/// a cheap no-op on a branch that's already correct.
pub fn plan(stacks: &Stacks, branches: &[String], store: &Store, repo: &Repo) -> Vec<Step> {
    stacks
        .ordered(branches)
        .into_iter()
        .filter(|e| e.parent.is_some() && !e.is_merged_or_closed())
        .map(|e| step_for(e, store, repo))
        .collect()
}

pub(crate) fn step_for(entry: &stack::Entry, store: &Store, repo: &Repo) -> Step {
    let (dir, location) = match &entry.holder {
        stack::Holder::Tree { id, name, .. } => match store.trees.iter().find(|t| t.id == *id) {
            Some(t) => (
                t.path.clone(),
                Location::Tree {
                    name: name.clone(),
                    provisioning: t.state == TreeState::Provisioning,
                },
            ),
            // `Holder::Tree` only ever comes from a store entry matching
            // this id (see `stack::load`); a miss means the store moved
            // under us since the stack was built. Falling back to base is
            // no worse than the `None` holder case below.
            None => (repo.base.clone(), Location::Base),
        },
        stack::Holder::Unregistered { path } => (path.clone(), Location::Unregistered),
        stack::Holder::Base | stack::Holder::None => (repo.base.clone(), Location::Base),
    };
    Step {
        branch: entry.branch.clone(),
        parent: entry.parent.clone(),
        dir,
        location,
    }
}

/// Reasons `step`'s directory isn't ready for a restack right now. Empty
/// means ready.
pub fn readiness(step: &Step) -> Vec<String> {
    let mut reasons = Vec::new();
    if let Location::Tree {
        provisioning: true, ..
    } = &step.location
    {
        reasons.push("still provisioning".to_string());
    }
    if git::is_dirty(&step.dir).unwrap_or(false) {
        reasons.push("uncommitted changes".to_string());
    }
    if git::rebase_or_merge_in_progress(&step.dir).unwrap_or(false) {
        reasons.push("mid-rebase or mid-merge".to_string());
    }
    reasons
}

/// `gt`'s wording for refusing a branch checked out elsewhere — the one
/// failure it reports with exit 0. Matched loosely on purpose: a reworded
/// message costs this extra detection, never turns a failure into a success,
/// because the exit code is still checked first.
const MISROUTED_MARKER: &str = "is checked out in worktree";

/// Pulls the worktree path out of `gt`'s own message so the failure we
/// report can name where the branch actually lives, not just that it does.
fn misrouted_worktree(output: &str) -> Option<String> {
    let after = output.split(MISROUTED_MARKER).nth(1)?;
    let line_end = after.find('\n').unwrap_or(after.len());
    let path = after[..line_end].trim().trim_end_matches('.');
    (!path.is_empty()).then(|| path.to_string())
}

enum StepOutcome {
    Restacked,
    Conflict,
    Misrouted { actual_dir: String },
}

fn run_one(gt_bin: &str, step: &Step) -> Result<StepOutcome> {
    let output = Command::new(gt_bin)
        .args([
            "restack",
            "--branch",
            &step.branch,
            "--only",
            "--no-interactive",
        ])
        .current_dir(&step.dir)
        .output()
        .with_context(|| format!("running gt restack --branch {} --only", step.branch))?;

    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));

    if output.status.success() {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if let Some(actual_dir) = misrouted_worktree(&combined) {
            return Ok(StepOutcome::Misrouted { actual_dir });
        }
        return Ok(StepOutcome::Restacked);
    }
    Ok(StepOutcome::Conflict)
}

/// What one walk did: every branch it actually restacked, and every branch
/// left needing one, paired with why.
#[derive(Debug, Default)]
pub struct WalkOutcome {
    pub restacked: Vec<String>,
    pub pending: Vec<(String, String)>,
}

impl WalkOutcome {
    /// Plain-English lines for a caller to print with its own prefix.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.restacked.is_empty() {
            lines.push(format!("restacked: {}", self.restacked.join(", ")));
        }
        for (branch, reason) in &self.pending {
            lines.push(format!("'{branch}' still needs a restack: {reason}"));
        }
        lines
    }
}

/// The result of attempting one branch's restack: either it succeeded, or
/// `gt` left it blocked for a reason a person has to resolve by hand
/// (a real conflict, or `gt` routing the restack to the wrong worktree).
pub enum RestackAttempt {
    Restacked,
    Blocked(String),
}

/// Runs `gt restack --only` for one step. A success updates `parent_revision`
/// to the parent it just restacked onto and clears the pending flag; a
/// blocked attempt marks the branch pending instead, since either way it
/// still needs a restack once the block is cleared.
pub(crate) fn restack_one_with(
    root: &Path,
    repo_name: &str,
    step: &Step,
    gt_bin: &str,
) -> Result<RestackAttempt> {
    println!("restacking '{}' in {}", step.branch, step.location.label());
    match run_one(gt_bin, step)? {
        StepOutcome::Restacked => {
            let parent_revision = step
                .parent
                .as_deref()
                .and_then(|p| git::rev_parse(&step.dir, p).ok());
            record_restacked(root, repo_name, &step.branch, parent_revision)?;
            Ok(RestackAttempt::Restacked)
        }
        StepOutcome::Misrouted { actual_dir } => {
            println!(
                "failed restacking '{}' ({})",
                step.branch,
                step.location.label()
            );
            println!(
                "  wt sent this to {}, but gt says '{}' is actually checked out in {actual_dir}",
                step.dir.display(),
                step.branch
            );
            println!(
                "  that's a wt bug, not a conflict for you to resolve — nothing ran, so \
                 there's nothing to continue"
            );
            mark_pending(root, repo_name, &step.branch)?;
            Ok(RestackAttempt::Blocked(
                "wt routed it to the wrong worktree".to_string(),
            ))
        }
        StepOutcome::Conflict => {
            println!(
                "failed restacking '{}' ({})",
                step.branch,
                step.location.label()
            );
            println!("  working directory: {}", step.dir.display());
            println!("  the tree is left mid-rebase; resolve the conflict there, then run:");
            println!("    cd {} && gt continue", step.dir.display());
            mark_pending(root, repo_name, &step.branch)?;
            Ok(RestackAttempt::Blocked(
                "left mid-rebase; resolve the conflict, then `gt continue`".to_string(),
            ))
        }
    }
}

/// Walks `steps` bottom-up. A branch whose directory isn't ready (dirty,
/// mid-rebase, still provisioning), or whose parent was skipped for the
/// same reason, is marked `pending_restack` in the store and left for a
/// later `wt sync` — the walk moves on to whatever else in `steps` doesn't
/// depend on it. A real `gt` conflict still stops the walk — that tree is
/// left for its agent to resolve with `gt continue` — and everything the
/// walk never reached is marked pending too, so the debt stays visible
/// instead of silently stale.
pub fn walk(root: &Path, repo_name: &str, steps: &[Step]) -> Result<WalkOutcome> {
    walk_with(root, repo_name, steps, "gt")
}

fn walk_with(root: &Path, repo_name: &str, steps: &[Step], gt_bin: &str) -> Result<WalkOutcome> {
    let mut blocked: HashSet<&str> = HashSet::new();
    let mut outcome = WalkOutcome::default();

    for (i, step) in steps.iter().enumerate() {
        if let Some(parent) = step.parent.as_deref()
            && blocked.contains(parent)
        {
            blocked.insert(&step.branch);
            mark_pending(root, repo_name, &step.branch)?;
            outcome.pending.push((
                step.branch.clone(),
                format!("its parent '{parent}' hasn't been restacked yet"),
            ));
            continue;
        }

        let reasons = readiness(step);
        if !reasons.is_empty() {
            blocked.insert(&step.branch);
            mark_pending(root, repo_name, &step.branch)?;
            outcome
                .pending
                .push((step.branch.clone(), reasons.join(", ")));
            continue;
        }

        match restack_one_with(root, repo_name, step, gt_bin)? {
            RestackAttempt::Restacked => outcome.restacked.push(step.branch.clone()),
            RestackAttempt::Blocked(reason) => {
                outcome.pending.push((step.branch.clone(), reason.clone()));
                mark_unreached(root, repo_name, &steps[i + 1..], &mut outcome)?;
                // The walk is about to end in an error, which carries only
                // this one branch's reason — print the full outcome first
                // so a caller still sees everything the abort left pending,
                // not just the branch that triggered it.
                for line in outcome.describe() {
                    println!("{line}");
                }
                bail!("restack stopped at '{}': {reason}", step.branch);
            }
        }
    }

    Ok(outcome)
}

/// Marks every step the walk never got to after it stopped partway through
/// — so an aborted walk still shows the full extent of what it left undone,
/// not just the one branch that actually failed.
fn mark_unreached(
    root: &Path,
    repo_name: &str,
    unreached: &[Step],
    outcome: &mut WalkOutcome,
) -> Result<()> {
    for step in unreached {
        mark_pending(root, repo_name, &step.branch)?;
        outcome.pending.push((
            step.branch.clone(),
            "the walk stopped before reaching it".to_string(),
        ));
    }
    Ok(())
}

fn mark_pending(root: &Path, repo_name: &str, branch: &str) -> Result<()> {
    store::with_store_lock(root, |s| {
        if let Some(t) = s
            .trees
            .iter_mut()
            .find(|t| t.repo == repo_name && t.branch == branch)
        {
            t.pending_restack = true;
        }
        Ok(())
    })
}

/// Records a branch's successful restack: the parent head it now sits on,
/// so the next walk's cheap prefilter can trust it again, and the pending
/// flag cleared since the debt it marked is paid.
fn record_restacked(
    root: &Path,
    repo_name: &str,
    branch: &str,
    parent_revision: Option<String>,
) -> Result<()> {
    store::with_store_lock(root, |s| {
        if let Some(t) = s
            .trees
            .iter_mut()
            .find(|t| t.repo == repo_name && t.branch == branch)
        {
            if let Some(rev) = parent_revision {
                t.parent_revision = Some(rev);
            }
            t.pending_restack = false;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command as StdCommand;

    use chrono::Utc;
    use uuid::Uuid;

    use crate::store::{Tree, TreeState};

    fn git(args: &[&str], cwd: &Path) {
        let out = StdCommand::new("git")
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

    fn sample_repo(base: PathBuf) -> Repo {
        Repo {
            base,
            last_fetch: None,
        }
    }

    fn write_pr_info(common_dir: &Path, branch: &str, state: &str) {
        fs::write(
            common_dir.join(".graphite_pr_info"),
            format!(
                r#"{{"prInfos": [{{"headRefName": "{branch}", "prNumber": 1,
                 "state": "{state}", "reviewDecision": null, "isDraft": false}}]}}"#
            ),
        )
        .unwrap();
    }

    /// `master -> a -> b -> c`, plus `d` (parent `a`, PR merged) and `e`
    /// (parent `a`, in an unregistered worktree). `a` is held by `tree-a`
    /// and `b` by `tree-b` — a different tree than its own parent — which
    /// is the shape that most needs the ordering to be right rather than
    /// merely alphabetical. `c` and `d` have tree rows but no worktree at
    /// all; `e`'s tree row is registered at a path that has drifted from
    /// where it's actually checked out.
    fn fixture() -> (PathBuf, Repo, Vec<Tree>, Stacks) {
        let dir = std::env::temp_dir().join(format!("wt-restack-test-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        git(&["init", "-q", "-b", "master"], &base);
        git(&["config", "user.email", "t@t"], &base);
        git(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);
        git(&["branch", "a"], &base);
        git(&["branch", "b"], &base);
        git(&["branch", "c"], &base);
        git(&["branch", "d"], &base);
        git(&["branch", "e"], &base);

        let tree_a_path = dir.join("tree-a");
        git(
            &["worktree", "add", tree_a_path.to_str().unwrap(), "a"],
            &base,
        );
        let tree_b_path = dir.join("tree-b");
        git(
            &["worktree", "add", tree_b_path.to_str().unwrap(), "b"],
            &base,
        );
        let tree_e_path = dir.join("tree-e-actual-checkout");
        git(
            &["worktree", "add", tree_e_path.to_str().unwrap(), "e"],
            &base,
        );

        write_pr_info(&base.join(".git"), "d", "MERGED");

        let repo = sample_repo(base.clone());
        let trees = vec![
            // `a`'s parent names trunk, which is never itself a tree — so
            // `a` is both the graph's root (no parent tree to group it
            // under) and still eligible to restack onto trunk.
            sample_tree(
                "tree-a",
                "a",
                Some("master"),
                fs::canonicalize(&tree_a_path).unwrap(),
            ),
            sample_tree(
                "tree-b",
                "b",
                Some("a"),
                fs::canonicalize(&tree_b_path).unwrap(),
            ),
            sample_tree(
                "tree-c",
                "c",
                Some("b"),
                dir.join("tree-c-never-checked-out"),
            ),
            sample_tree(
                "tree-d",
                "d",
                Some("a"),
                dir.join("tree-d-never-checked-out"),
            ),
            sample_tree(
                "tree-e",
                "e",
                Some("a"),
                dir.join("tree-e-stale-registration"),
            ),
        ];

        let mut store = Store::default();
        store.repos.insert("r".to_string(), repo.clone());
        store.trees = trees.clone();

        let stacks = stack::load("r", &repo, &store).unwrap().unwrap();
        (dir, repo, trees, stacks)
    }

    fn all_branches() -> Vec<String> {
        vec![
            "master".into(),
            "a".into(),
            "b".into(),
            "c".into(),
            "d".into(),
            "e".into(),
        ]
    }

    #[test]
    fn plan_orders_bottom_up_even_when_parent_and_child_live_in_different_trees() {
        let (dir, repo, trees, stacks) = fixture();
        let store = Store {
            trees,
            ..Default::default()
        };

        let steps = plan(&stacks, &all_branches(), &store, &repo);
        let pos = |b: &str| steps.iter().position(|s| s.branch == b).unwrap();

        assert!(pos("a") < pos("b"), "a must restack before its child b");
        assert!(pos("b") < pos("c"), "b must restack before its child c");
        assert!(pos("a") < pos("e"), "a must restack before its child e");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_skips_trunk_and_merged_branches() {
        let (dir, repo, trees, stacks) = fixture();
        let store = Store {
            trees,
            ..Default::default()
        };

        let steps = plan(&stacks, &all_branches(), &store, &repo);
        let branches: Vec<&str> = steps.iter().map(|s| s.branch.as_str()).collect();

        assert!(
            !branches.contains(&"master"),
            "trunk has no parent to restack onto"
        );
        assert!(!branches.contains(&"d"), "d's PR is merged");
        assert!(branches.contains(&"a"));
        assert!(branches.contains(&"b"));
        assert!(branches.contains(&"c"));
        assert!(branches.contains(&"e"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_resolves_every_kind_of_holder_to_the_right_directory() {
        let (dir, repo, trees, stacks) = fixture();
        let tree_a_path = trees[0].path.clone();
        let tree_b_path = trees[1].path.clone();
        let store = Store {
            trees,
            ..Default::default()
        };

        let steps = plan(&stacks, &all_branches(), &store, &repo);
        let by_branch: std::collections::HashMap<&str, &Step> =
            steps.iter().map(|s| (s.branch.as_str(), s)).collect();

        assert_eq!(by_branch["a"].dir, tree_a_path);
        assert!(
            matches!(by_branch["a"].location, Location::Tree { ref name, .. } if name == "tree-a")
        );

        assert_eq!(by_branch["b"].dir, tree_b_path);
        assert!(
            matches!(by_branch["b"].location, Location::Tree { ref name, .. } if name == "tree-b")
        );

        // `c` is checked out nowhere: falls back to the repo's base clone.
        assert_eq!(by_branch["c"].dir, repo.base);
        assert!(matches!(by_branch["c"].location, Location::Base));

        // `e` sits in a worktree `wt` never registered.
        assert!(matches!(by_branch["e"].location, Location::Unregistered));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn readiness_flags_dirty_mid_rebase_and_provisioning_but_not_a_clean_tree() {
        let dir = std::env::temp_dir().join(format!("wt-restack-preflight-{}", Uuid::now_v7()));
        let clean = dir.join("clean");
        let dirty = dir.join("dirty");
        let rebasing = dir.join("rebasing");
        let provisioning = dir.join("provisioning");
        for d in [&clean, &dirty, &rebasing, &provisioning] {
            fs::create_dir_all(d).unwrap();
            git(&["init", "-q", "-b", "master"], d);
            git(&["config", "user.email", "t@t"], d);
            git(&["config", "user.name", "t"], d);
            fs::write(d.join("f.txt"), "0\n").unwrap();
            git(&["add", "-A"], d);
            git(&["commit", "-qm", "init"], d);
        }
        fs::write(dirty.join("f.txt"), "uncommitted\n").unwrap();

        git(&["checkout", "-qb", "feature"], &rebasing);
        fs::write(rebasing.join("f.txt"), "feature\n").unwrap();
        git(&["commit", "-aqm", "feature"], &rebasing);
        git(&["checkout", "-q", "master"], &rebasing);
        fs::write(rebasing.join("f.txt"), "master\n").unwrap();
        git(&["commit", "-aqm", "master"], &rebasing);
        StdCommand::new("git")
            .args(["merge", "feature"])
            .current_dir(&rebasing)
            .output()
            .unwrap();

        let steps = [
            Step {
                branch: "clean-branch".into(),
                parent: Some("master".into()),
                dir: clean.clone(),
                location: Location::Base,
            },
            Step {
                branch: "dirty-branch".into(),
                parent: Some("master".into()),
                dir: dirty.clone(),
                location: Location::Tree {
                    name: "dirty-tree".into(),
                    provisioning: false,
                },
            },
            Step {
                branch: "rebasing-branch".into(),
                parent: Some("master".into()),
                dir: rebasing.clone(),
                location: Location::Tree {
                    name: "rebasing-tree".into(),
                    provisioning: false,
                },
            },
            Step {
                branch: "provisioning-branch".into(),
                parent: Some("master".into()),
                dir: provisioning.clone(),
                location: Location::Tree {
                    name: "provisioning-tree".into(),
                    provisioning: true,
                },
            },
        ];

        assert!(
            readiness(&steps[0]).is_empty(),
            "clean-branch must be ready"
        );
        assert_eq!(
            readiness(&steps[1]),
            vec!["uncommitted changes".to_string()]
        );
        // A conflicted merge leaves unmerged paths, which git status also
        // reports as dirty — both reasons are expected, not just one.
        assert!(
            readiness(&steps[2]).contains(&"mid-rebase or mid-merge".to_string()),
            "reasons were: {:?}",
            readiness(&steps[2])
        );
        assert_eq!(readiness(&steps[3]), vec!["still provisioning".to_string()]);

        fs::remove_dir_all(&dir).ok();
    }

    fn fake_gt(dir: &Path, log: &Path, fail_branch: &str) -> PathBuf {
        let script = dir.join("gt");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nbranch=\"$3\"\necho \"$branch $(pwd)\" >> \"{}\"\n\
                 if [ \"$branch\" = \"{fail_branch}\" ]; then\n  echo conflict >&2\n  exit 1\nfi\nexit 0\n",
                log.display(),
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    #[test]
    fn execute_stops_at_the_first_failure_and_runs_nothing_after() {
        let dir = std::env::temp_dir().join(format!("wt-restack-exec-{}", Uuid::now_v7()));
        let dir_a = dir.join("a");
        let dir_b = dir.join("b");
        let dir_c = dir.join("c");
        for d in [&dir_a, &dir_b, &dir_c] {
            fs::create_dir_all(d).unwrap();
        }
        let log = dir.join("log.txt");
        let gt = fake_gt(&dir, &log, "b");

        let steps = vec![
            Step {
                branch: "a".into(),
                parent: Some("master".into()),
                dir: dir_a,
                location: Location::Base,
            },
            Step {
                branch: "b".into(),
                parent: Some("a".into()),
                dir: dir_b.clone(),
                location: Location::Tree {
                    name: "tree-b".into(),
                    provisioning: false,
                },
            },
            Step {
                branch: "c".into(),
                parent: Some("b".into()),
                dir: dir_c,
                location: Location::Base,
            },
        ];

        let root = dir.join("wtroot");
        store::with_store_lock(&root, |s| {
            s.repos.insert("r".to_string(), sample_repo(dir.clone()));
            s.trees = vec![
                sample_tree("tree-a", "a", Some("master"), steps[0].dir.clone()),
                sample_tree("tree-b", "b", Some("a"), dir_b),
                sample_tree("tree-c", "c", Some("b"), steps[2].dir.clone()),
            ];
            Ok(())
        })
        .unwrap();

        let err = walk_with(&root, "r", &steps, gt.to_str().unwrap()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains('b'), "message was: {msg}");

        let log_contents = fs::read_to_string(&log).unwrap();
        assert!(log_contents.contains("a "), "log was: {log_contents}");
        assert!(log_contents.contains("b "), "log was: {log_contents}");
        assert!(!log_contents.contains("c "), "log was: {log_contents}");

        // The abort still leaves a full record behind: the branch that
        // failed and everything after it, not just the one named in the
        // error `walk` returns.
        let store = store::load(&root).unwrap();
        let by_branch = |b: &str| store.trees.iter().find(|t| t.branch == b).unwrap();
        assert!(!by_branch("a").pending_restack, "a restacked cleanly");
        assert!(
            by_branch("b").pending_restack,
            "b is where the conflict hit"
        );
        assert!(by_branch("c").pending_restack, "c was never reached");

        fs::remove_dir_all(&dir).ok();
    }

    /// Prints `gt`'s real wording for refusing a branch checked out
    /// elsewhere and exits 0 — the exact combination that must still stop
    /// the walk despite the success status.
    fn fake_gt_misrouted(dir: &Path, branch: &str, actual_dir: &str) -> PathBuf {
        let script = dir.join("gt");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nbranch=\"$3\"\nif [ \"$branch\" = \"{branch}\" ]; then\n  \
                 echo \"Did not restack branch $branch because it is checked out in worktree \
                 {actual_dir}.\"\n  exit 0\nfi\nexit 0\n",
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    #[test]
    fn execute_treats_a_misrouted_worktree_message_as_a_failure_despite_exit_zero() {
        let dir =
            std::env::temp_dir().join(format!("wt-restack-exec-misrouted-{}", Uuid::now_v7()));
        let dir_a = dir.join("a");
        let dir_b = dir.join("b");
        for d in [&dir_a, &dir_b] {
            fs::create_dir_all(d).unwrap();
        }
        let actual_dir = dir.join("wherever-it-really-lives");
        let gt = fake_gt_misrouted(&dir, "a", actual_dir.to_str().unwrap());

        let steps = vec![
            Step {
                branch: "a".into(),
                parent: Some("master".into()),
                dir: dir_a,
                location: Location::Base,
            },
            Step {
                branch: "b".into(),
                parent: Some("a".into()),
                dir: dir_b,
                location: Location::Base,
            },
        ];

        let root = dir.join("wtroot");
        let err = walk_with(&root, "r", &steps, gt.to_str().unwrap()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("wrong worktree"), "message was: {msg}");
        assert!(msg.contains('a'), "message was: {msg}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn misrouted_worktree_extracts_the_path_gt_names() {
        let output =
            "Did not restack branch a because it is checked out in worktree /tmp/tree-a.\n";
        assert_eq!(misrouted_worktree(output), Some("/tmp/tree-a".to_string()));
        assert_eq!(misrouted_worktree("Restacked a on master.\n"), None);
    }

    #[test]
    fn execute_runs_every_step_when_none_fail() {
        let dir = std::env::temp_dir().join(format!("wt-restack-exec-ok-{}", Uuid::now_v7()));
        let dir_a = dir.join("a");
        let dir_b = dir.join("b");
        for d in [&dir_a, &dir_b] {
            fs::create_dir_all(d).unwrap();
        }
        let log = dir.join("log.txt");
        let gt = fake_gt(&dir, &log, "nothing-matches-this");

        let steps = vec![
            Step {
                branch: "a".into(),
                parent: Some("master".into()),
                dir: dir_a,
                location: Location::Base,
            },
            Step {
                branch: "b".into(),
                parent: Some("a".into()),
                dir: dir_b,
                location: Location::Base,
            },
        ];

        let root = dir.join("wtroot");
        let outcome = walk_with(&root, "r", &steps, gt.to_str().unwrap()).unwrap();
        assert_eq!(outcome.restacked, vec!["a".to_string(), "b".to_string()]);
        assert!(outcome.pending.is_empty());

        let log_contents = fs::read_to_string(&log).unwrap();
        assert!(log_contents.contains("a "), "log was: {log_contents}");
        assert!(log_contents.contains("b "), "log was: {log_contents}");

        fs::remove_dir_all(&dir).ok();
    }

    /// `a -> b -> c`, plus sibling `d` (parent `a` too). `b`'s directory is
    /// dirty; `c` sits on top of it. Neither should stop `a` and `d`, which
    /// share nothing with the blocked branch.
    #[test]
    fn walk_marks_a_blocked_branch_and_its_upstack_pending_but_restacks_unrelated_siblings() {
        let dir = std::env::temp_dir().join(format!("wt-restack-walk-skip-{}", Uuid::now_v7()));
        let dir_a = dir.join("a");
        let dir_b = dir.join("b-dirty");
        let dir_c = dir.join("c");
        let dir_d = dir.join("d");
        for d in [&dir_a, &dir_b, &dir_c, &dir_d] {
            fs::create_dir_all(d).unwrap();
        }
        git(&["init", "-q", "-b", "master"], &dir_b);
        git(&["config", "user.email", "t@t"], &dir_b);
        git(&["config", "user.name", "t"], &dir_b);
        fs::write(dir_b.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &dir_b);
        git(&["commit", "-qm", "init"], &dir_b);
        fs::write(dir_b.join("f.txt"), "uncommitted\n").unwrap();

        let root = dir.join("wtroot");
        store::with_store_lock(&root, |s| {
            s.repos.insert("r".to_string(), sample_repo(dir.clone()));
            s.trees = vec![
                sample_tree("tree-a", "a", Some("master"), dir_a.clone()),
                sample_tree("tree-b", "b", Some("a"), dir_b.clone()),
                sample_tree("tree-c", "c", Some("b"), dir_c.clone()),
                sample_tree("tree-d", "d", Some("a"), dir_d.clone()),
            ];
            Ok(())
        })
        .unwrap();

        let log = dir.join("log.txt");
        let gt = fake_gt(&dir, &log, "nothing-matches-this");

        let steps = vec![
            Step {
                branch: "a".into(),
                parent: Some("master".into()),
                dir: dir_a,
                location: Location::Tree {
                    name: "tree-a".into(),
                    provisioning: false,
                },
            },
            Step {
                branch: "b".into(),
                parent: Some("a".into()),
                dir: dir_b,
                location: Location::Tree {
                    name: "tree-b".into(),
                    provisioning: false,
                },
            },
            Step {
                branch: "c".into(),
                parent: Some("b".into()),
                dir: dir_c,
                location: Location::Tree {
                    name: "tree-c".into(),
                    provisioning: false,
                },
            },
            Step {
                branch: "d".into(),
                parent: Some("a".into()),
                dir: dir_d,
                location: Location::Tree {
                    name: "tree-d".into(),
                    provisioning: false,
                },
            },
        ];

        let outcome = walk_with(&root, "r", &steps, gt.to_str().unwrap()).unwrap();

        assert_eq!(outcome.restacked, vec!["a".to_string(), "d".to_string()]);
        let pending_branches: Vec<&str> = outcome.pending.iter().map(|(b, _)| b.as_str()).collect();
        assert_eq!(pending_branches, vec!["b", "c"]);
        assert!(
            outcome.pending[0].1.contains("uncommitted changes"),
            "b's reason was: {}",
            outcome.pending[0].1
        );
        assert!(
            outcome.pending[1]
                .1
                .contains("its parent 'b' hasn't been restacked yet"),
            "c's reason was: {}",
            outcome.pending[1].1
        );

        let store = store::load(&root).unwrap();
        let by_branch = |b: &str| store.trees.iter().find(|t| t.branch == b).unwrap();
        assert!(!by_branch("a").pending_restack, "a restacked cleanly");
        assert!(by_branch("b").pending_restack, "b was dirty");
        assert!(by_branch("c").pending_restack, "c's parent never restacked");
        assert!(!by_branch("d").pending_restack, "d shares nothing with b");

        fs::remove_dir_all(&dir).ok();
    }
}
