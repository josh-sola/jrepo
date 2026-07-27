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

pub fn worktree_remove(base: &Path, tree_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let path_str = tree_path.to_string_lossy().to_string();
    args.push(&path_str);
    let out = run(&args, base)?;
    if !out.status.success() {
        bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
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

/// `None` means no upstream is configured, not that the tree is clean.
pub fn upstream_ref(path: &Path) -> Option<String> {
    stdout_trimmed(
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
        path,
    )
    .ok()
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
