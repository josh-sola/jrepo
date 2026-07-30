use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::Utc;

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
pub fn sync(root: &Path, repo_filter: Option<String>, stack: bool) -> Result<()> {
    let store = store::load(root)?;
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
        match sync_one(root, name, repo) {
            Ok(line) => {
                println!("{name}: {line}");
                if stack && let Err(e) = sync_stack(root, name, repo) {
                    had_failure = true;
                    println!("{name}: {e:#}");
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

fn sync_one(root: &Path, name: &str, repo: &Repo) -> Result<String> {
    let trunk_ref = format!("origin/{}", repo.trunk);
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

    let dirty = git::status_porcelain(&repo.base)?;
    if !dirty.is_empty() {
        bail!(
            "dirty, refusing to fast-forward ({}): {}",
            dirty.len(),
            dirty.join("; ")
        );
    }

    let branch = git::current_branch(&repo.base)?;
    if branch != repo.trunk {
        return Ok(format!(
            "{fetch_desc}; on branch '{branch}', not '{}' — skipping fast-forward",
            repo.trunk
        ));
    }

    let head = git::rev_parse(&repo.base, "HEAD").context("resolving HEAD")?;
    if head == after {
        return Ok(format!("{fetch_desc}; trunk unchanged"));
    }

    git::merge_ff_only(&repo.base, &trunk_ref)?;
    Ok(format!(
        "{fetch_desc}; fast-forwarded {} to {}",
        repo.trunk,
        &after[..after.len().min(7)]
    ))
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
            trunk: "master".into(),
            branch_prefix: "josh/".into(),
            last_fetch: None,
            shared: Vec::new(),
            copy: Vec::new(),
            env: Default::default(),
            steps: Vec::new(),
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
}
