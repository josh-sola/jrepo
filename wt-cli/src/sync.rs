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

/// Restacks every stack in `repo` that has branches held by more than one
/// worktree — a single-tree stack has no cross-worktree problem for this
/// walk to solve. Stops at the first failure, same as `wt restack`, leaving
/// later stacks in the repo unwalked rather than pressing on past a tree
/// left mid-rebase.
fn sync_stack(root: &Path, name: &str, repo: &Repo) -> Result<()> {
    let store = store::load(root)?;
    let Some(stacks) = stack::load(name, repo, &store)? else {
        println!("{name}: no Graphite stack info, skipping the restack walk");
        return Ok(());
    };

    let to_walk = stacks_to_walk(&stacks, &store, repo);
    for (r, steps) in &to_walk {
        let offenders = restack::preflight(steps);
        if !offenders.is_empty() {
            for o in &offenders {
                println!(
                    "{name}: refusing to restack the stack rooted at '{r}' — {} ({}): {}",
                    o.label,
                    o.dir.display(),
                    o.reasons.join(", ")
                );
            }
            bail!(
                "stack rooted at '{r}' has {} tree(s) not ready for a restack",
                offenders.len()
            );
        }
        restack::execute(steps)?;
    }

    let walked = to_walk.len();
    println!(
        "{name}: {}",
        if walked == 0 {
            "no multi-tree stacks to restack".to_string()
        } else {
            format!(
                "restacked {walked} multi-tree stack{}",
                if walked == 1 { "" } else { "s" }
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

    /// Two independent stacks off the same repo: `master -> s1a`, held by a
    /// single tree, and `root2 -> s2a -> s2b`, split across two. Only the
    /// second has a cross-worktree problem for `--stack` to solve.
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
        for b in ["s1a", "root2", "s2a", "s2b"] {
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
             ('master', NULL, 'TRUNK'), ('s1a', 'master', NULL), ('root2', NULL, NULL), \
             ('s2a', 'root2', NULL), ('s2b', 's2a', NULL);",
        );

        let repo = Repo {
            base: base.clone(),
            last_fetch: None,
        };

        let mut store = Store::default();
        store.repos.insert("r".to_string(), repo.clone());
        store.trees = vec![
            sample_tree("tree-s1a", "s1a", fs::canonicalize(&tree_s1a).unwrap()),
            sample_tree("tree-s2a", "s2a", fs::canonicalize(&tree_s2a).unwrap()),
            sample_tree("tree-s2b", "s2b", fs::canonicalize(&tree_s2b).unwrap()),
        ];

        let stacks = stack::load("r", &repo, &store).unwrap().unwrap();
        (dir, repo, store, stacks)
    }

    #[test]
    fn stacks_to_walk_skips_a_single_tree_stack_and_keeps_a_multi_tree_one() {
        let (dir, repo, store, stacks) = fixture();

        let walked = stacks_to_walk(&stacks, &store, &repo);
        assert_eq!(
            walked.len(),
            1,
            "only root2's stack spans more than one tree"
        );
        let (root, steps) = &walked[0];
        assert_eq!(root, "root2");
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
}
