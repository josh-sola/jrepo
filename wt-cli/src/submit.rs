//! Wraps `gt submit`. wt preflights the branches it's about to push against
//! its own restack-debt tracking, runs `gt` non-interactively in the
//! resolved tree, then records the PR numbers `gt` assigned.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::git;
use crate::graphite;
use crate::restack;
use crate::stack::{self, Stacks};
use crate::store::{self, Repo, Store};
use crate::sync;

pub struct SubmitOptions {
    /// `gt`'s `--stack`: submit upstack descendants too, not just the
    /// resolved branch and its ancestors.
    pub stack: bool,
    pub draft: bool,
    pub publish: bool,
}

pub fn submit(root: &Path, selector: Option<String>, opts: SubmitOptions) -> Result<()> {
    submit_with(root, selector, opts, "gt")
}

fn submit_with(
    root: &Path,
    selector: Option<String>,
    opts: SubmitOptions,
    gt_bin: &str,
) -> Result<()> {
    let store = store::load(root)?;
    let tree = sync::resolve_tree(&store, selector.as_deref())?;
    let repo = store
        .repos
        .get(&tree.repo)
        .with_context(|| format!("repo '{}' is not registered", tree.repo))?;
    let branch = store::live_branch(tree).unwrap_or_else(|| tree.branch.clone());

    let stacks = stack::load(&tree.repo, repo, &store)?.with_context(|| {
        format!("'{branch}' has no wt tree; only branches made with `wt new` or `wt pr new` can be submitted")
    })?;
    if !stacks.graph.contains(&branch) {
        bail!(
            "'{branch}' has no wt tree; only branches made with `wt new` or `wt pr new` can be \
             submitted"
        );
    }

    let scope = if opts.stack {
        stacks.graph.stack(&branch)
    } else {
        let mut scope = stacks.graph.downstack(&branch);
        scope.push(branch.clone());
        scope
    };
    check_restack_debt(&stacks, &scope)?;
    check_submitting_tree_is_idle(&stacks, &store, repo, &branch)?;

    let mut args = vec!["submit", "--no-interactive", "--no-edit"];
    if opts.stack {
        args.push("--stack");
    }
    if opts.draft {
        args.push("--draft");
    }
    if opts.publish {
        args.push("--publish");
    }

    let tree_path = tree.path.clone();
    // Inherits this process's stdout/stderr so `gt`'s own prompts and
    // progress show up live, the same as running it by hand.
    let status = Command::new(gt_bin)
        .args(&args)
        .current_dir(&tree_path)
        .status()
        .with_context(|| format!("running gt submit in {}", tree_path.display()))?;
    if !status.success() {
        bail!(
            "gt submit failed in {} (see its output above)",
            tree_path.display()
        );
    }

    record_pr_numbers(root, &tree.repo, repo)
}

/// Refuses to submit if any branch in `scope` might still need a restack —
/// pushing a stale base risks a broken PR chain, so this is an error, not a
/// warning `wt submit` presses on past.
fn check_restack_debt(stacks: &Stacks, scope: &[String]) -> Result<()> {
    let offenders: Vec<String> = stacks
        .ordered(scope)
        .into_iter()
        .filter(|e| e.shows_needs_restack())
        .map(|e| format!("'{}' in {}", e.branch, holder_label(&e.holder)))
        .collect();
    if offenders.is_empty() {
        return Ok(());
    }
    let (noun, verb) = if offenders.len() == 1 {
        ("branch", "needs")
    } else {
        ("branches", "need")
    };
    bail!(
        "refusing to submit: {noun} still {verb} a restack — run `wt restack` or `wt sync` \
         there first:\n  {}",
        offenders.join("\n  ")
    );
}

/// `gt submit` runs in the tree holding `branch`; uncommitted changes or a
/// mid-rebase there could get force-pushed or block on a prompt despite
/// `--no-interactive`.
fn check_submitting_tree_is_idle(
    stacks: &Stacks,
    store: &Store,
    repo: &Repo,
    branch: &str,
) -> Result<()> {
    let Some(entry) = stacks.get(branch) else {
        return Ok(());
    };
    let step = restack::step_for(entry, store, repo);
    let reasons = restack::readiness(&step);
    if reasons.is_empty() {
        return Ok(());
    }
    bail!(
        "refusing to submit from {}: {}",
        step.location.label(),
        reasons.join(", ")
    );
}

fn holder_label(h: &stack::Holder) -> String {
    match h {
        stack::Holder::Tree { name, .. } => format!("tree \"{name}\""),
        stack::Holder::Base => "the repo's base checkout".to_string(),
        stack::Holder::Unregistered { path } => {
            format!("an unregistered worktree at {}", path.display())
        }
        stack::Holder::None => "no worktree right now".to_string(),
    }
}

/// `gt submit` updates `.graphite_pr_info` itself; this only reads it back,
/// never `gt`'s own output, so a future wording change in `gt` can't break
/// which PRs wt thinks exist.
fn record_pr_numbers(root: &Path, repo_name: &str, repo: &Repo) -> Result<()> {
    let common_dir = git::common_dir(&repo.base)?;
    let Some(pr_infos) = graphite::read_pr_info(&common_dir) else {
        return Ok(());
    };
    store::with_store_lock(root, |s| {
        for t in s.trees.iter_mut().filter(|t| t.repo == repo_name) {
            if let Some(pr) = pr_infos.get(&t.branch) {
                t.pr_number = Some(pr.pr_number);
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command as StdCommand;

    use chrono::Utc;
    use uuid::Uuid;

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

    fn sample_tree(
        name: &str,
        branch: &str,
        parent_branch: Option<&str>,
        path: PathBuf,
    ) -> store::Tree {
        store::Tree {
            id: Uuid::now_v7(),
            repo: "r".into(),
            name: name.into(),
            branch: branch.into(),
            path,
            created: Utc::now(),
            state: store::TreeState::Ready,
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

    struct Fixture {
        dir: PathBuf,
        root: PathBuf,
        repo: Repo,
        path_b: PathBuf,
    }

    /// `a -> b -> c`, each held by its own clean worktree, no restack debt
    /// anywhere — the baseline every test starts from and mutates.
    fn fixture() -> Fixture {
        let dir = std::env::temp_dir().join(format!("wt-submit-test-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        git(&["init", "-q", "-b", "master"], &base);
        git(&["config", "user.email", "t@t"], &base);
        git(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);
        for b in ["a", "b", "c"] {
            git(&["branch", b], &base);
        }

        let path_a = dir.join("tree-a");
        git(&["worktree", "add", path_a.to_str().unwrap(), "a"], &base);
        let path_b = dir.join("tree-b");
        git(&["worktree", "add", path_b.to_str().unwrap(), "b"], &base);
        let path_c = dir.join("tree-c");
        git(&["worktree", "add", path_c.to_str().unwrap(), "c"], &base);

        let root = dir.join("wtroot");
        let repo = Repo {
            base: base.clone(),
            last_fetch: None,
        };
        store::with_store_lock(&root, |s| {
            s.repos.insert("r".to_string(), repo.clone());
            s.trees = vec![
                sample_tree("tree-a", "a", Some("master"), path_a),
                sample_tree("tree-b", "b", Some("a"), path_b.clone()),
                sample_tree("tree-c", "c", Some("b"), path_c),
            ];
            Ok(())
        })
        .unwrap();

        Fixture {
            dir,
            root,
            repo,
            path_b,
        }
    }

    fn fake_gt(dir: &Path, common_dir: &Path, log: &Path) -> PathBuf {
        let script = dir.join("gt");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" >> \"{}\"\ncat > \"{}/.graphite_pr_info\" << 'EOF'\n\
                 {{\"prInfos\": [{{\"headRefName\": \"a\", \"prNumber\": 10, \"state\": \"OPEN\", \
                 \"reviewDecision\": null, \"isDraft\": true}}, {{\"headRefName\": \"b\", \
                 \"prNumber\": 11, \"state\": \"OPEN\", \"reviewDecision\": null, \"isDraft\": \
                 true}}]}}\nEOF\nexit 0\n",
                log.display(),
                common_dir.display(),
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    /// Never actually runs: any invocation appends to `log`, so a test can
    /// prove a preflight failure stopped `wt submit` before it shelled out.
    fn fake_gt_that_must_not_run(dir: &Path, log: &Path) -> PathBuf {
        let script = dir.join("gt");
        fs::write(
            &script,
            format!("#!/bin/sh\necho called >> \"{}\"\nexit 0\n", log.display()),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    #[test]
    fn submit_refuses_when_a_downstack_branch_needs_a_restack() {
        let f = fixture();
        store::with_store_lock(&f.root, |s| {
            s.trees
                .iter_mut()
                .find(|t| t.branch == "a")
                .unwrap()
                .pending_restack = true;
            Ok(())
        })
        .unwrap();

        let log = f.dir.join("gt-log.txt");
        let gt = fake_gt_that_must_not_run(&f.dir, &log);
        let err = submit_with(
            &f.root,
            Some("tree-b".to_string()),
            SubmitOptions {
                stack: false,
                draft: false,
                publish: false,
            },
            gt.to_str().unwrap(),
        )
        .unwrap_err();

        assert!(err.to_string().contains('a'), "message was: {err}");
        assert!(!log.exists(), "gt must never run after a preflight refusal");
        fs::remove_dir_all(&f.dir).ok();
    }

    #[test]
    fn submit_ignores_upstack_debt_unless_stack_is_requested() {
        let f = fixture();
        store::with_store_lock(&f.root, |s| {
            s.trees
                .iter_mut()
                .find(|t| t.branch == "c")
                .unwrap()
                .pending_restack = true;
            Ok(())
        })
        .unwrap();

        let log = f.dir.join("gt-log.txt");
        let gt = fake_gt(&f.dir, &f.repo.base.join(".git"), &log);

        // `c` is upstack of `b`: a plain submit of `b` never looks at it.
        submit_with(
            &f.root,
            Some("tree-b".to_string()),
            SubmitOptions {
                stack: false,
                draft: false,
                publish: false,
            },
            gt.to_str().unwrap(),
        )
        .unwrap();
        assert!(log.exists(), "gt should have run for the narrow submit");
        fs::remove_file(&log).ok();

        // Asking for --stack pulls `c` into scope, so its debt now blocks.
        let err = submit_with(
            &f.root,
            Some("tree-b".to_string()),
            SubmitOptions {
                stack: true,
                draft: false,
                publish: false,
            },
            gt.to_str().unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains('c'), "message was: {err}");
        assert!(!log.exists(), "gt must not run once --stack sees c's debt");

        fs::remove_dir_all(&f.dir).ok();
    }

    #[test]
    fn submit_refuses_when_the_submitting_tree_is_dirty() {
        let f = fixture();
        fs::write(f.path_b.join("f.txt"), "uncommitted\n").unwrap();

        let log = f.dir.join("gt-log.txt");
        let gt = fake_gt_that_must_not_run(&f.dir, &log);
        let err = submit_with(
            &f.root,
            Some("tree-b".to_string()),
            SubmitOptions {
                stack: false,
                draft: false,
                publish: false,
            },
            gt.to_str().unwrap(),
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("uncommitted"),
            "message was: {err}"
        );
        assert!(!log.exists(), "gt must never run against a dirty tree");
        fs::remove_dir_all(&f.dir).ok();
    }

    #[test]
    fn submit_passes_the_right_flags_and_never_restacks_then_records_pr_numbers() {
        let f = fixture();
        let log = f.dir.join("gt-log.txt");
        let gt = fake_gt(&f.dir, &f.repo.base.join(".git"), &log);

        submit_with(
            &f.root,
            Some("tree-b".to_string()),
            SubmitOptions {
                stack: true,
                draft: true,
                publish: false,
            },
            gt.to_str().unwrap(),
        )
        .unwrap();

        let invocation = fs::read_to_string(&log).unwrap();
        assert!(
            invocation.contains("submit --no-interactive --no-edit --stack --draft"),
            "invocation was: {invocation}"
        );
        assert!(
            !invocation.contains("--restack"),
            "wt submit must never restack a branch another tree holds: {invocation}"
        );
        assert!(
            !invocation.contains("--publish"),
            "invocation was: {invocation}"
        );

        let store = store::load(&f.root).unwrap();
        let by_branch = |b: &str| store.trees.iter().find(|t| t.branch == b).unwrap();
        assert_eq!(by_branch("a").pr_number, Some(10));
        assert_eq!(by_branch("b").pr_number, Some(11));
        assert_eq!(by_branch("c").pr_number, None);

        fs::remove_dir_all(&f.dir).ok();
    }
}
