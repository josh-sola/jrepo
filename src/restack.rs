//! Drives `gt restack --branch <b> --only` across every worktree that holds
//! a branch in a Graphite stack. `gt` refuses to touch a branch checked out
//! in another worktree, so `wt` — the only thing that can see every
//! worktree at once — has to run each branch's restack from wherever it
//! actually lives, bottom-up.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::git;
use crate::stack::{self, Stacks};
use crate::store::{Repo, Store, TreeState};

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

fn step_for(entry: &stack::Entry, store: &Store, repo: &Repo) -> Step {
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

/// One working directory the plan would run in that isn't ready for it.
pub struct Offender {
    pub label: String,
    pub dir: PathBuf,
    pub reasons: Vec<String>,
}

/// Checks every distinct directory the plan touches before any step runs,
/// so the walk either goes cleanly or not at all — never dies halfway with
/// some branches restacked and others not.
pub fn preflight(steps: &[Step]) -> Vec<Offender> {
    let mut checked: HashSet<&PathBuf> = HashSet::new();
    let mut offenders = Vec::new();
    for step in steps {
        if !checked.insert(&step.dir) {
            continue;
        }

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
        if !reasons.is_empty() {
            offenders.push(Offender {
                label: step.location.label(),
                dir: step.dir.clone(),
                reasons,
            });
        }
    }
    offenders
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

/// Runs every step in order, printing each as it starts. Stops at the first
/// failure and leaves that tree exactly where `gt` left it — mid-rebase, if
/// that's what happened — rather than auto-continuing or auto-aborting a
/// conflict that's the user's to resolve.
pub fn execute(steps: &[Step]) -> Result<()> {
    execute_with(steps, "gt")
}

fn execute_with(steps: &[Step], gt_bin: &str) -> Result<()> {
    for step in steps {
        println!("restacking '{}' in {}", step.branch, step.location.label());
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
                bail!(
                    "restack stopped at '{}': wt routed it to the wrong worktree",
                    step.branch
                );
            }
            continue;
        }

        println!(
            "failed restacking '{}' ({})",
            step.branch,
            step.location.label()
        );
        println!("  working directory: {}", step.dir.display());
        println!("  the tree is left mid-rebase; resolve the conflict there, then run:");
        println!("    cd {} && gt continue", step.dir.display());
        bail!("restack stopped at '{}'", step.branch);
    }
    Ok(())
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

    fn sqlite(db: &Path, sql: &str) {
        let out = StdCommand::new("/usr/bin/sqlite3")
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
        }
    }

    fn sample_repo(base: PathBuf) -> Repo {
        Repo {
            base,
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            last_fetch: None,
            shared: Vec::new(),
            copy: Vec::new(),
            env: Default::default(),
            steps: Vec::new(),
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
    /// merely alphabetical.
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
        let tree_e_path = dir.join("tree-e-unregistered");
        git(
            &["worktree", "add", tree_e_path.to_str().unwrap(), "e"],
            &base,
        );

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
             ('c', 'b', NULL), ('d', 'a', NULL), ('e', 'a', NULL);",
        );
        write_pr_info(&common_dir, "d", "MERGED");

        let repo = sample_repo(base.clone());
        let tree_a = sample_tree("tree-a", "a", fs::canonicalize(&tree_a_path).unwrap());
        let tree_b = sample_tree("tree-b", "b", fs::canonicalize(&tree_b_path).unwrap());
        let trees = vec![tree_a, tree_b];

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
    fn preflight_flags_dirty_mid_rebase_and_provisioning_trees_but_not_a_clean_one() {
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

        let steps = vec![
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

        let offenders = preflight(&steps);
        let by_label: std::collections::HashMap<&str, &Offender> =
            offenders.iter().map(|o| (o.label.as_str(), o)).collect();

        assert_eq!(
            offenders.len(),
            3,
            "clean-branch's [base] dir must not be flagged"
        );
        assert!(
            by_label["dirty-tree"]
                .reasons
                .contains(&"uncommitted changes".to_string())
        );
        assert!(
            by_label["rebasing-tree"]
                .reasons
                .contains(&"mid-rebase or mid-merge".to_string())
        );
        assert!(
            by_label["provisioning-tree"]
                .reasons
                .contains(&"still provisioning".to_string())
        );
        assert!(
            !offenders
                .iter()
                .any(|o| o.dir == clean && o.label == "[base]")
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preflight_only_checks_each_directory_once() {
        let dir =
            std::env::temp_dir().join(format!("wt-restack-preflight-dedup-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        git(&["init", "-q", "-b", "master"], &dir);
        git(&["config", "user.email", "t@t"], &dir);
        git(&["config", "user.name", "t"], &dir);
        fs::write(dir.join("f.txt"), "uncommitted\n").unwrap();

        let steps = vec![
            Step {
                branch: "one".into(),
                parent: Some("master".into()),
                dir: dir.clone(),
                location: Location::Base,
            },
            Step {
                branch: "two".into(),
                parent: Some("one".into()),
                dir: dir.clone(),
                location: Location::Base,
            },
        ];

        let offenders = preflight(&steps);
        assert_eq!(offenders.len(), 1);

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

        let err = execute_with(&steps, gt.to_str().unwrap()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains('b'), "message was: {msg}");

        let log_contents = fs::read_to_string(&log).unwrap();
        assert!(log_contents.contains("a "), "log was: {log_contents}");
        assert!(log_contents.contains("b "), "log was: {log_contents}");
        assert!(!log_contents.contains("c "), "log was: {log_contents}");

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

        let err = execute_with(&steps, gt.to_str().unwrap()).unwrap_err();
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

        execute_with(&steps, gt.to_str().unwrap()).unwrap();
        let log_contents = fs::read_to_string(&log).unwrap();
        assert!(log_contents.contains("a "), "log was: {log_contents}");
        assert!(log_contents.contains("b "), "log was: {log_contents}");

        fs::remove_dir_all(&dir).ok();
    }
}
