use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

fn run(args: &[&str], cwd: &Path) -> Result<Output> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))
}

fn ok(args: &[&str], cwd: &Path) -> Result<bool> {
    Ok(run(args, cwd)?.status.success())
}

fn stdout_trimmed(args: &[&str], cwd: &Path) -> Result<String> {
    let out = run(args, cwd)?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn is_git_repo(path: &Path) -> Result<bool> {
    ok(&["rev-parse", "--git-dir"], path)
}

pub fn has_origin_remote(path: &Path) -> Result<bool> {
    ok(&["remote", "get-url", "origin"], path)
}

/// Falls back to `master` — a repo with no remote HEAD pointer (or none
/// reachable yet) still needs a trunk name to build worktree commands.
pub fn trunk_branch(path: &Path) -> String {
    stdout_trimmed(&["symbolic-ref", "refs/remotes/origin/HEAD"], path)
        .ok()
        .and_then(|s| s.rsplit('/').next().map(str::to_string))
        .unwrap_or_else(|| "master".to_string())
}

/// The common git dir, not `.git` — a linked worktree's own git dir is a
/// private subdirectory, but `info/exclude` must be shared by every
/// worktree of the clone.
pub fn common_dir(path: &Path) -> Result<std::path::PathBuf> {
    let s = stdout_trimmed(
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        path,
    )?;
    Ok(std::path::PathBuf::from(s))
}

pub fn fetch_prune(path: &Path) -> Result<()> {
    let out = run(&["fetch", "--prune"], path)?;
    if !out.status.success() {
        bail!(
            "git fetch --prune failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn branch_exists_local(path: &Path, branch: &str) -> Result<bool> {
    ok(
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
        path,
    )
}

pub fn branch_exists_remote(path: &Path, branch: &str) -> Result<bool> {
    ok(
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{branch}"),
        ],
        path,
    )
}

pub fn worktree_add(base: &Path, tree_path: &Path, branch: &str, start_point: &str) -> Result<()> {
    let out = run(
        &[
            "worktree",
            "add",
            &tree_path.to_string_lossy(),
            "-b",
            branch,
            start_point,
        ],
        base,
    )?;
    if !out.status.success() {
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// `git worktree add` copies the main worktree's `config.worktree` into
/// every new linked worktree's admin dir when `extensions.worktreeConfig`
/// is on (verified empirically) — so a `core.hooksPath` set on base to
/// block commits there arrives already active in every fresh tree unless
/// something clears it. A missing key here is normal, not an error:
/// `--unset-all` exits non-zero when there was nothing copied.
pub fn clear_worktree_hooks_path(tree_path: &Path) -> Result<()> {
    run(
        &["config", "--worktree", "--unset-all", "core.hooksPath"],
        tree_path,
    )?;
    Ok(())
}

pub fn worktree_prune(base: &Path) -> Result<()> {
    let out = run(&["worktree", "prune"], base)?;
    if !out.status.success() {
        bail!(
            "git worktree prune failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn is_dirty(path: &Path) -> Result<bool> {
    let out = run(&["status", "--porcelain"], path)?;
    if !out.status.success() {
        bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(!out.stdout.is_empty())
}

pub fn status_porcelain(path: &Path) -> Result<Vec<String>> {
    let out = run(&["status", "--porcelain"], path)?;
    if !out.status.success() {
        bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

pub fn current_branch(path: &Path) -> Result<String> {
    stdout_trimmed(&["rev-parse", "--abbrev-ref", "HEAD"], path)
}

pub fn rev_parse(path: &Path, rev: &str) -> Result<String> {
    stdout_trimmed(&["rev-parse", rev], path)
}

pub fn merge_ff_only(path: &Path, rev: &str) -> Result<()> {
    let out = run(&["merge", "--ff-only", rev], path)?;
    if !out.status.success() {
        bail!(
            "git merge --ff-only {rev} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// `scope` is `--local` or `--worktree`. `None` means unset, not an error —
/// `git config --get` exits non-zero for a missing key.
pub fn config_get(path: &Path, scope: &str, key: &str) -> Option<String> {
    let out = run(&["config", scope, "--get", key], path).ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn config_set(path: &Path, scope: &str, key: &str, value: &str) -> Result<()> {
    let out = run(&["config", scope, key, value], path)?;
    if !out.status.success() {
        bail!(
            "git config {scope} {key} {value} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// `None` means no upstream is configured, not that the branch is clean.
/// Resolves the branch's own upstream by name rather than `HEAD`'s, so it
/// works from `base` even when `branch` isn't the currently checked-out ref
/// there (or the tree that had it checked out is already gone).
pub fn branch_upstream(path: &Path, branch: &str) -> Option<String> {
    stdout_trimmed(
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            &format!("{branch}@{{upstream}}"),
        ],
        path,
    )
    .ok()
}

pub fn delete_branch(base: &Path, branch: &str) -> Result<()> {
    let out = run(&["branch", "-D", branch], base)?;
    if !out.status.success() {
        bail!(
            "git branch -D {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub struct WorktreeEntry {
    pub path: std::path::PathBuf,
    pub branch: Option<String>,
}

/// Porcelain output is blocks of `key value` lines separated by a blank
/// line, one block per worktree; a bare or detached entry has no `branch`
/// line at all, which is why `branch` is optional rather than defaulted.
pub fn worktree_list(base: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = stdout_trimmed(&["worktree", "list", "--porcelain"], base)?;
    let mut entries = Vec::new();
    let mut path = None;
    let mut branch = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(path) = path.take() {
                entries.push(WorktreeEntry {
                    path,
                    branch: branch.take(),
                });
            }
            path = Some(std::path::PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.trim_start_matches("refs/heads/").to_string());
        }
    }
    if let Some(path) = path.take() {
        entries.push(WorktreeEntry { path, branch });
    }
    Ok(entries)
}

/// `--directory` folds a wholly-ignored directory (`node_modules/`,
/// `.venv/`, a build cache) into a single entry ending in `/` instead of
/// listing every file inside it. Without that flag this call would
/// enumerate hundreds of thousands of cache files to find a dozen ignored
/// `.env` files.
pub fn ignored_files(path: &Path) -> Result<Vec<String>> {
    let out = run(
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
        path,
    )?;
    if !out.status.success() {
        bail!(
            "git ls-files --others --ignored failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout
        .split('\0')
        .filter(|s| !s.is_empty() && !s.ends_with('/'))
        .map(str::to_string)
        .collect())
}

/// `refs/stash` lives in the common git dir, so a stash taken in one
/// worktree is visible from every other worktree of the same clone — this
/// is what lets `wt adopt` push here and pop in `stash_pop` against a tree
/// that didn't exist yet when the stash was made.
/// Returns the new stash commit so the caller can prove which entry is its
/// own before popping.
pub fn stash_push_include_untracked(path: &Path, message: &str) -> Result<String> {
    let out = run(
        &["stash", "push", "--include-untracked", "-m", message],
        path,
    )?;
    if !out.status.success() {
        bail!(
            "git stash push failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    stash_top(path)?.context("git stash push reported success but left no stash entry")
}

fn stash_top(path: &Path) -> Result<Option<String>> {
    let out = run(&["rev-parse", "--verify", "--quiet", "stash@{0}"], path)?;
    if !out.status.success() {
        return Ok(None);
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(if sha.is_empty() { None } else { Some(sha) })
}

/// A failed pop (conflict, or the target tree already dirty) leaves the
/// stash entry in place on its own — git only drops it after a clean apply
/// — so the caller needs no extra step to keep it intact on failure.
///
/// `expected` must be the stash commit this caller pushed. `git stash pop`
/// always takes the newest entry, so without the check an unrelated stash
/// pushed in the meantime would be popped into the tree instead.
pub fn stash_pop(path: &Path, expected: &str) -> Result<()> {
    match stash_top(path)? {
        Some(top) if top == expected => {}
        Some(top) => bail!(
            "refusing to pop: the newest stash is {top}, not the {expected} pushed for this \
             tree; recover by hand with `git stash list` and `git stash pop <entry>`"
        ),
        None => bail!("refusing to pop: the stash pushed for this tree ({expected}) is gone"),
    }

    let out = run(&["stash", "pop"], path)?;
    if !out.status.success() {
        bail!(
            "git stash pop failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn commits_ahead(path: &Path, range: &str) -> Result<bool> {
    let out = run(&["log", "--oneline", range], path)?;
    if !out.status.success() {
        bail!(
            "git log {range} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(!out.stdout.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn fixture_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wt-git-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q", "."],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            run(&args, &dir).unwrap();
        }
        fs::write(dir.join("tracked.txt"), "one\n").unwrap();
        run(&["add", "-A"], &dir).unwrap();
        run(&["commit", "-qm", "init"], &dir).unwrap();
        dir
    }

    #[test]
    fn stash_pop_refuses_when_a_newer_unrelated_stash_arrived() {
        let repo = fixture_repo();
        fs::write(repo.join("tracked.txt"), "ours\n").unwrap();
        let ours = stash_push_include_untracked(&repo, "ours").unwrap();

        fs::write(repo.join("tracked.txt"), "theirs\n").unwrap();
        let theirs = stash_push_include_untracked(&repo, "theirs").unwrap();
        assert_ne!(ours, theirs);

        let err = stash_pop(&repo, &ours).unwrap_err().to_string();
        assert!(err.contains("refusing to pop"), "unexpected error: {err}");
        assert_eq!(stash_top(&repo).unwrap().as_deref(), Some(theirs.as_str()));

        fs::remove_dir_all(&repo).ok();
    }
}
