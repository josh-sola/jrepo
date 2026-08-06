use std::fs;
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

/// A hot spare has no branch of its own: whoever claims it creates the
/// branch they asked for, and a detached HEAD in the meantime keeps the
/// spare out of the user's branch namespace and out of Graphite's graph.
pub fn worktree_add_detached(base: &Path, tree_path: &Path, start_point: &str) -> Result<()> {
    let out = run(
        &[
            "worktree",
            "add",
            "--detach",
            &tree_path.to_string_lossy(),
            start_point,
        ],
        base,
    )?;
    if !out.status.success() {
        bail!(
            "git worktree add --detach failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// The cutover that turns a spare into a real tree. Failure is expected and
/// survivable — a dirty working tree left by a half-finished provisioning
/// step lands here — so callers fall back to building a tree from cold
/// rather than treating it as fatal.
pub fn switch_new_branch(tree_path: &Path, branch: &str, start_point: &str) -> Result<()> {
    let out = run(&["switch", "-c", branch, start_point], tree_path)?;
    if !out.status.success() {
        bail!(
            "git switch -c {branch} {start_point} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn checkout_detached(tree_path: &Path, rev: &str) -> Result<()> {
    let out = run(&["checkout", "--detach", rev], tree_path)?;
    if !out.status.success() {
        bail!(
            "git checkout --detach {rev} failed: {}",
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
    status_porcelain_filtered(path, SubmoduleFilter::None)
}

/// `Dirty` hides a submodule's own uncommitted changes but still reports a
/// stale gitlink; `All` hides both. Neither affects a diff in the
/// superproject itself.
pub enum SubmoduleFilter {
    None,
    #[allow(dead_code)]
    Dirty,
    All,
}

impl SubmoduleFilter {
    fn arg(&self) -> Option<&'static str> {
        match self {
            SubmoduleFilter::None => None,
            SubmoduleFilter::Dirty => Some("--ignore-submodules=dirty"),
            SubmoduleFilter::All => Some("--ignore-submodules=all"),
        }
    }
}

pub fn status_porcelain_filtered(path: &Path, filter: SubmoduleFilter) -> Result<Vec<String>> {
    let mut args = vec!["status", "--porcelain"];
    if let Some(arg) = filter.arg() {
        args.push(arg);
    }
    let out = run(&args, path)?;
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

pub struct SubmoduleEntry {
    pub path: String,
    pub state: SubmoduleState,
}

pub enum SubmoduleState {
    InSync,
    StalePointer,
    Uninitialized,
    Conflicted,
}

/// Parses `git submodule status` — a leading ` `, `+`, `-`, or `U` per line,
/// then the submodule's checked-out sha, then its path, then an optional
/// `(describe)` suffix this ignores.
pub fn submodule_status(path: &Path) -> Result<Vec<SubmoduleEntry>> {
    let out = run(&["submodule", "status"], path)?;
    if !out.status.success() {
        bail!(
            "git submodule status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let prefix = line
                .chars()
                .next()
                .with_context(|| format!("parsing submodule status line: {line}"))?;
            let state = match prefix {
                ' ' => SubmoduleState::InSync,
                '+' => SubmoduleState::StalePointer,
                '-' => SubmoduleState::Uninitialized,
                'U' => SubmoduleState::Conflicted,
                other => bail!("unrecognized submodule status prefix '{other}' in line: {line}"),
            };
            let sub_path = line[1..]
                .split_whitespace()
                .nth(1)
                .with_context(|| format!("parsing submodule status line: {line}"))?
                .to_string();
            Ok(SubmoduleEntry {
                path: sub_path,
                state,
            })
        })
        .collect()
}

pub fn submodule_update_recursive(path: &Path) -> Result<()> {
    let out = run(&["submodule", "update", "--init", "--recursive"], path)?;
    if !out.status.success() {
        bail!(
            "git submodule update --init --recursive failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
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

/// Canonicalized so a caller can join the result against `Tree.path`, which
/// `tree.rs` canonicalizes at creation — otherwise a symlinked component
/// (`/tmp` vs `/private/tmp`) would make an exact-path match miss even
/// though both sides name the same worktree.
pub fn worktree_branches(cwd: &Path) -> Result<Vec<(std::path::PathBuf, Option<String>)>> {
    Ok(worktree_list(cwd)?
        .into_iter()
        .map(|w| {
            let path = fs::canonicalize(&w.path).unwrap_or(w.path);
            (path, w.branch)
        })
        .collect())
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

/// True when `path`'s worktree is mid-rebase or mid-merge — state that
/// would block starting another rebase there. Read from the worktree's own
/// git dir (`--git-dir`, not `--git-common-dir`): rebase and merge state is
/// per worktree, not shared across a clone's linked worktrees.
pub fn rebase_or_merge_in_progress(path: &Path) -> Result<bool> {
    let git_dir = stdout_trimmed(&["rev-parse", "--path-format=absolute", "--git-dir"], path)?;
    let git_dir = Path::new(&git_dir);
    Ok(git_dir.join("rebase-merge").exists()
        || git_dir.join("rebase-apply").exists()
        || git_dir.join("MERGE_HEAD").exists())
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

/// Compares patch-ids rather than SHAs, so a commit a squash merge landed
/// under a different SHA on `upstream` still counts as landed.
pub fn unlanded_commits(path: &Path, upstream: &str, head: &str) -> Result<Vec<String>> {
    let out = run(&["cherry", upstream, head], path)?;
    if !out.status.success() {
        bail!(
            "git cherry {upstream} {head} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("+ "))
        .map(str::to_string)
        .collect())
}

/// Count of commits reachable from `range`'s right side but not its left,
/// e.g. `HEAD..origin/main` for how far behind a checkout's trunk is.
pub fn rev_list_count(path: &Path, range: &str) -> Result<usize> {
    let out = stdout_trimmed(&["rev-list", "--count", range], path)?;
    out.parse()
        .with_context(|| format!("parsing `git rev-list --count {range}` output: {out}"))
}

pub fn log_oneline(path: &Path, range: &str, limit: usize) -> Result<Vec<String>> {
    let out = run(&["log", "--oneline", "-n", &limit.to_string(), range], path)?;
    if !out.status.success() {
        bail!(
            "git log {range} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
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
    fn rebase_or_merge_in_progress_is_false_on_a_clean_repo() {
        let repo = fixture_repo();
        assert!(!rebase_or_merge_in_progress(&repo).unwrap());
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn rebase_or_merge_in_progress_detects_a_conflicted_merge() {
        let repo = fixture_repo();
        let base_branch = current_branch(&repo).unwrap();
        run(&["checkout", "-qb", "feature"], &repo).unwrap();
        fs::write(repo.join("tracked.txt"), "feature\n").unwrap();
        run(&["commit", "-aqm", "feature edit"], &repo).unwrap();
        run(&["checkout", "-q", &base_branch], &repo).unwrap();
        fs::write(repo.join("tracked.txt"), "base\n").unwrap();
        run(&["commit", "-aqm", "base edit"], &repo).unwrap();

        run(&["merge", "feature"], &repo).unwrap(); // conflicts; leaves MERGE_HEAD
        assert!(rebase_or_merge_in_progress(&repo).unwrap());

        fs::remove_dir_all(&repo).ok();
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

    /// A base repo with `sub` added as a submodule at HEAD. Local-path
    /// submodules need `protocol.file.allow=always` — git refuses `file://`
    /// submodules otherwise.
    fn submodule_fixture() -> (PathBuf, PathBuf) {
        let sub = fixture_repo();
        let base = fixture_repo();
        run(
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                sub.to_str().unwrap(),
                "sub",
            ],
            &base,
        )
        .unwrap();
        run(&["commit", "-qm", "add submodule"], &base).unwrap();
        (base, sub)
    }

    #[test]
    fn submodule_status_reports_in_sync_stale_and_uninitialized() {
        let (base, sub) = submodule_fixture();

        assert!(matches!(
            submodule_status(&base).unwrap()[0].state,
            SubmoduleState::InSync
        ));

        fs::write(base.join("sub").join("tracked.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "advance"], &base.join("sub")).unwrap();
        let entries = submodule_status(&base).unwrap();
        assert_eq!(entries[0].path, "sub");
        assert!(matches!(entries[0].state, SubmoduleState::StalePointer));

        run(&["submodule", "deinit", "-f", "sub"], &base).unwrap();
        let entries = submodule_status(&base).unwrap();
        assert!(matches!(entries[0].state, SubmoduleState::Uninitialized));

        fs::remove_dir_all(&base).ok();
        fs::remove_dir_all(&sub).ok();
    }

    #[test]
    fn ignore_submodules_flags_distinguish_stale_pointer_from_dirty_submodule_work() {
        let (base, sub) = submodule_fixture();

        // A stale gitlink with an otherwise clean submodule stays visible
        // under `dirty` but disappears under `all`.
        fs::write(base.join("sub").join("tracked.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "advance"], &base.join("sub")).unwrap();
        assert!(
            !status_porcelain_filtered(&base, SubmoduleFilter::Dirty)
                .unwrap()
                .is_empty()
        );
        assert!(
            status_porcelain_filtered(&base, SubmoduleFilter::All)
                .unwrap()
                .is_empty()
        );

        run(&["add", "sub"], &base).unwrap();
        run(&["commit", "-qm", "bump sub"], &base).unwrap();

        // Uncommitted work inside the submodule disappears even under
        // `dirty`, since the checked-out commit itself matches the index.
        fs::write(base.join("sub").join("tracked.txt"), "edited\n").unwrap();
        assert!(!status_porcelain(&base).unwrap().is_empty());
        assert!(
            status_porcelain_filtered(&base, SubmoduleFilter::Dirty)
                .unwrap()
                .is_empty()
        );
        assert!(
            status_porcelain_filtered(&base, SubmoduleFilter::All)
                .unwrap()
                .is_empty()
        );

        // A change in the base itself is never hidden.
        run(&["checkout", "--", "tracked.txt"], &base.join("sub")).unwrap();
        fs::write(base.join("tracked.txt"), "edited\n").unwrap();
        assert!(
            !status_porcelain_filtered(&base, SubmoduleFilter::All)
                .unwrap()
                .is_empty()
        );

        fs::remove_dir_all(&base).ok();
        fs::remove_dir_all(&sub).ok();
    }
}
