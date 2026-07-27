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
