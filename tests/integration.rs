//! End-to-end coverage against throwaway git repos under the OS temp dir.
//! Nothing here touches a real clone — every fixture is created fresh and
//! `WT_ROOT` is passed per-process so parallel tests never collide.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use uuid::Uuid;

fn wt_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_wt"))
}

fn unique_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wt-cli-it-{label}-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} in {} failed:\nstdout: {}\nstderr: {}",
        args,
        cwd.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A throwaway "origin" plus a work clone with one commit, wired up so
/// `git symbolic-ref refs/remotes/origin/HEAD` resolves without network
/// access — `wt init`'s trunk detection depends on that ref existing.
fn fixture_repo(dir: &Path) -> PathBuf {
    let bare = dir.join("origin.git");
    let work = dir.join("work");
    git(&["init", "--bare", "--quiet", bare.to_str().unwrap()], dir);
    // `git init --bare` may default HEAD to `main`; pin it to the branch
    // we actually push so `remote set-head -a` can resolve it locally.
    git(&["symbolic-ref", "HEAD", "refs/heads/master"], &bare);
    git(
        &["init", "--quiet", "-b", "master", work.to_str().unwrap()],
        dir,
    );
    std::fs::write(work.join("README.md"), "fixture\n").unwrap();
    git(&["add", "-A"], &work);
    git(
        &[
            "-c",
            "user.email=wt-cli-test@example.com",
            "-c",
            "user.name=wt-cli-test",
            "commit",
            "-q",
            "-m",
            "init",
        ],
        &work,
    );
    git(&["remote", "add", "origin", bare.to_str().unwrap()], &work);
    git(&["push", "-q", "origin", "master"], &work);
    git(&["fetch", "-q", "origin"], &work);
    git(&["remote", "set-head", "origin", "-a"], &work);
    work
}

fn run_wt(root: &Path, args: &[&str]) -> Output {
    Command::new(wt_bin())
        .args(args)
        .env("WT_ROOT", root)
        .output()
        .expect("spawn wt")
}

fn assert_success(out: &Output, label: &str) {
    assert!(
        out.status.success(),
        "{label} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_new_ls_rm_round_trip() {
    let tmp = unique_dir("e2e");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    let out = run_wt(
        &root,
        &["init", "myrepo", "--adopt", base.to_str().unwrap()],
    );
    assert_success(&out, "init");

    let out = run_wt(&root, &["new", "myrepo", "--name", "scratch test"]);
    assert_success(&out, "new");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tree_path = PathBuf::from(stdout.lines().last().unwrap().trim());
    assert!(
        tree_path.join(".git").exists(),
        "worktree not created at {}",
        tree_path.display()
    );

    // Provisioning finishes in the background; a fixture with zero steps
    // is a race without this, not a guaranteed pass.
    assert_success(&run_wt(&root, &["wait", "scratch test"]), "wait");

    let out = run_wt(&root, &["ls", "--json"]);
    assert_success(&out, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let entries = entries.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "scratch test");
    assert_eq!(entries[0]["state"], "ready");
    assert_eq!(entries[0]["dirty"], false);

    let out = run_wt(&root, &["path", "scratch test"]);
    assert_success(&out, "path");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        tree_path.to_str().unwrap()
    );

    let out = run_wt(&root, &["rm", "scratch test"]);
    assert_success(&out, "rm");
    assert!(
        !tree_path.exists(),
        "tree path still on disk after rm: {}",
        tree_path.display()
    );

    let out = run_wt(&root, &["ls", "--json"]);
    assert_success(&out, "ls --json after rm");
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 0);
}

fn git_status_porcelain(path: &Path) -> String {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .expect("spawn git status");
    assert!(
        out.status.success(),
        "git status --porcelain failed in {}",
        path.display()
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A directory-only `.gitignore` pattern (`local/`) does not match a
/// symlink named `local`; only `info/exclude` (which gets the trailing
/// slash stripped) does. This proves both base and the tree stay clean
/// even though the fixture's `.gitignore` only uses the trailing-slash form.
#[test]
fn shared_symlinks_stay_invisible_to_git_status_in_base_and_tree() {
    let tmp = unique_dir("gitignore-symlink");
    let bare = tmp.join("origin.git");
    let work = tmp.join("work");
    git(&["init", "--bare", "--quiet", bare.to_str().unwrap()], &tmp);
    git(&["symbolic-ref", "HEAD", "refs/heads/master"], &bare);
    git(
        &["init", "--quiet", "-b", "master", work.to_str().unwrap()],
        &tmp,
    );

    std::fs::write(work.join("README.md"), "fixture\n").unwrap();
    std::fs::write(work.join(".gitignore"), "local/\nplans/\n").unwrap();
    std::fs::write(work.join(".worktreeinclude"), "local/\nplans/\n").unwrap();
    std::fs::create_dir_all(work.join("local")).unwrap();
    std::fs::write(work.join("local").join("note.md"), "local state\n").unwrap();
    std::fs::create_dir_all(work.join("plans")).unwrap();
    std::fs::write(work.join("plans").join("todo.md"), "a plan\n").unwrap();

    git(&["add", "-A"], &work);
    git(
        &[
            "-c",
            "user.email=wt-cli-test@example.com",
            "-c",
            "user.name=wt-cli-test",
            "commit",
            "-q",
            "-m",
            "init",
        ],
        &work,
    );
    git(&["remote", "add", "origin", bare.to_str().unwrap()], &work);
    git(&["push", "-q", "origin", "master"], &work);
    git(&["fetch", "-q", "origin"], &work);
    git(&["remote", "set-head", "origin", "-a"], &work);

    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", work.to_str().unwrap()],
        ),
        "init",
    );

    let base_status = git_status_porcelain(&work);
    assert!(
        base_status.is_empty(),
        "base git status not clean after init:\n{base_status}"
    );

    let out = run_wt(&root, &["new", "myrepo", "--name", "gitignore check"]);
    assert_success(&out, "new");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tree_path = PathBuf::from(stdout.lines().last().unwrap().trim());
    assert_success(&run_wt(&root, &["wait", "gitignore check"]), "wait");

    let tree_status = git_status_porcelain(&tree_path);
    assert!(
        tree_status.is_empty(),
        "tree git status not clean after new:\n{tree_status}"
    );
}

#[test]
fn new_with_unslugifiable_name_errors_clearly() {
    let tmp = unique_dir("badslug");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    let out = run_wt(&root, &["new", "myrepo", "--name", "???"]);
    assert!(
        !out.status.success(),
        "expected 'wt new --name ???' to fail without --branch"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--branch"),
        "expected a hint to pass --branch, got: {stderr}"
    );

    let out = run_wt(&root, &["ls", "--json"]);
    assert_success(&out, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        0,
        "no tree should be registered after the guard rejects the name"
    );
}

#[test]
fn name_matches_longest_registered_path_prefix() {
    let tmp = unique_dir("name");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let out = run_wt(&root, &["new", "myrepo", "--name", "name lookup test"]);
    assert_success(&out, "new");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tree_path = PathBuf::from(stdout.lines().last().unwrap().trim());

    let out = run_wt(&root, &["name", "--path", tree_path.to_str().unwrap()]);
    assert_success(&out, "name at tree root");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "name lookup test"
    );

    let subdir = tree_path.join("some").join("nested").join("dir");
    std::fs::create_dir_all(&subdir).unwrap();
    let out = run_wt(&root, &["name", "--path", subdir.to_str().unwrap()]);
    assert_success(&out, "name at subdirectory");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "name lookup test"
    );

    let outside = unique_dir("outside");
    let out = run_wt(&root, &["name", "--path", outside.to_str().unwrap()]);
    assert_success(&out, "name outside any tree");
    assert!(
        out.stdout.is_empty(),
        "expected no output, got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A fixture whose base has a real `.gitmodules`, so `wt init` detects a
/// submodule provisioning step and any tree made from it can reproduce
/// `git worktree remove`'s submodule refusal once the submodule is checked
/// out inside that tree.
fn fixture_repo_with_submodule(dir: &Path) -> PathBuf {
    let sub_bare = dir.join("sub.git");
    let sub_seed = dir.join("sub-seed");
    git(
        &["init", "--bare", "--quiet", sub_bare.to_str().unwrap()],
        dir,
    );
    git(&["symbolic-ref", "HEAD", "refs/heads/master"], &sub_bare);
    git(
        &[
            "init",
            "--quiet",
            "-b",
            "master",
            sub_seed.to_str().unwrap(),
        ],
        dir,
    );
    std::fs::write(sub_seed.join("f.txt"), "sub\n").unwrap();
    git(&["add", "-A"], &sub_seed);
    git(
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
        &sub_seed,
    );
    git(
        &["push", "-q", sub_bare.to_str().unwrap(), "master"],
        &sub_seed,
    );

    let base = fixture_repo(dir);
    git(
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub_bare.to_str().unwrap(),
            "vendor/sub",
        ],
        &base,
    );
    git(
        &[
            "-c",
            "user.email=wt-cli-test@example.com",
            "-c",
            "user.name=wt-cli-test",
            "commit",
            "-q",
            "-m",
            "add submodule",
        ],
        &base,
    );
    git(&["push", "-q", "origin", "master"], &base);
    git(&["fetch", "-q", "origin"], &base);
    base
}

#[test]
fn rm_deinits_submodules_before_removing_worktree() {
    let tmp = unique_dir("submodule-rm");
    let base = fixture_repo_with_submodule(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    // Skip the auto-detected submodule step (tagged "node"); the test
    // initializes the submodule itself so the scenario is deterministic.
    let out = run_wt(
        &root,
        &[
            "new",
            "myrepo",
            "--name",
            "submodule tree",
            "--profile",
            "none",
        ],
    );
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "submodule tree"]), "wait");

    let init_out = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "--recursive",
        ])
        .current_dir(&tree_path)
        .output()
        .expect("spawn submodule update");
    assert!(
        init_out.status.success(),
        "submodule update --init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );
    assert!(tree_path.join("vendor/sub/f.txt").exists());

    let plain_remove = Command::new("git")
        .args(["worktree", "remove", tree_path.to_str().unwrap()])
        .current_dir(&base)
        .output()
        .expect("spawn git worktree remove");
    assert!(
        !plain_remove.status.success(),
        "expected a plain 'git worktree remove' to refuse a tree with a checked-out submodule"
    );

    assert_success(&run_wt(&root, &["rm", "submodule tree"]), "rm");
    assert!(
        !tree_path.exists(),
        "tree still on disk after 'wt rm': {}",
        tree_path.display()
    );

    let out = run_wt(&root, &["ls", "--json"]);
    assert_success(&out, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 0);
}

#[test]
fn rm_keeps_registry_entry_when_removal_genuinely_fails() {
    let tmp = unique_dir("rm-fail");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let out = run_wt(&root, &["new", "myrepo", "--name", "locked tree"]);
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "locked tree"]), "wait");

    let status = Command::new("chflags")
        .args(["uchg", tree_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "chflags uchg failed");

    let out = run_wt(&root, &["rm", "locked tree"]);
    Command::new("chflags")
        .args(["nouchg", tree_path.to_str().unwrap()])
        .status()
        .unwrap();

    assert!(
        !out.status.success(),
        "expected 'wt rm' to fail while the tree is immutable"
    );
    assert!(
        tree_path.exists(),
        "tree should still be on disk after a failed removal"
    );

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        1,
        "the registry entry must survive a genuine removal failure, not just drift"
    );

    // Clean up directly rather than through `wt rm` again: the interrupted
    // `git worktree remove` can leave git's own worktree bookkeeping (not
    // wt's) in a state this test has no need to reason about.
    std::fs::remove_dir_all(&tree_path).ok();
    Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(&base)
        .status()
        .unwrap();
    run_wt(&root, &["rm", "locked tree"]);
}

#[test]
fn rm_unregisters_drifted_tree_whose_path_is_already_gone() {
    let tmp = unique_dir("rm-drift");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let out = run_wt(&root, &["new", "myrepo", "--name", "drifted tree"]);
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "drifted tree"]), "wait");

    // Simulate drift: something outside wt deleted the directory directly.
    std::fs::remove_dir_all(&tree_path).unwrap();

    assert_success(
        &run_wt(&root, &["rm", "drifted tree"]),
        "rm on a drifted tree",
    );

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        0,
        "a genuinely gone path must still be unregistered"
    );
}

#[test]
fn rm_delete_branch_removes_branch_only_when_safe() {
    let tmp = unique_dir("rm-branch");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let out = run_wt(&root, &["new", "myrepo", "--name", "branch cleanup"]);
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "branch cleanup"]), "wait");
    let branch = "josh/branch-cleanup";
    assert!(git_branch_exists(&base, branch));

    assert_success(
        &run_wt(&root, &["rm", "branch cleanup", "--delete-branch"]),
        "rm --delete-branch",
    );
    assert!(!tree_path.exists());
    assert!(
        !git_branch_exists(&base, branch),
        "branch should be deleted when it has no unpushed commits"
    );

    // A second tree, but with a local commit never pushed anywhere: the
    // branch must survive `--delete-branch` even though `--force` allows
    // the worktree itself to be torn down.
    let out = run_wt(
        &root,
        &[
            "new",
            "myrepo",
            "--name",
            "branch keep",
            "--branch",
            "josh/branch-keep",
        ],
    );
    assert_success(&out, "new");
    let tree_path2 = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "branch keep"]), "wait");
    std::fs::write(tree_path2.join("unpushed.txt"), "work\n").unwrap();
    git(&["add", "-A"], &tree_path2);
    git(
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "unpushed work",
        ],
        &tree_path2,
    );

    assert_success(
        &run_wt(&root, &["rm", "branch keep", "--force", "--delete-branch"]),
        "rm --force --delete-branch",
    );
    assert!(!tree_path2.exists());
    assert!(
        git_branch_exists(&base, "josh/branch-keep"),
        "branch with unpushed commits must survive --delete-branch"
    );

    git(&["branch", "-D", "josh/branch-keep"], &base);
}

fn git_branch_exists(base: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(base)
        .status()
        .unwrap()
        .success()
}

#[test]
fn gc_dry_run_reports_then_real_run_reaps_clean_tree_and_deletes_branch() {
    let tmp = unique_dir("gc");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let out = run_wt(&root, &["new", "myrepo", "--name", "gc clean"]);
    assert_success(&out, "new");
    let clean_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "gc clean"]), "wait");

    let out = run_wt(&root, &["new", "myrepo", "--name", "gc ahead"]);
    assert_success(&out, "new");
    let ahead_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "gc ahead"]), "wait");
    std::fs::write(ahead_path.join("more.txt"), "work\n").unwrap();
    git(&["add", "-A"], &ahead_path);
    git(
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "ahead of trunk",
        ],
        &ahead_path,
    );

    let dry = run_wt(&root, &["gc", "--dry-run"]);
    assert_success(&dry, "gc --dry-run");
    let dry_stdout = String::from_utf8_lossy(&dry.stdout);
    assert!(
        dry_stdout.contains("gc clean"),
        "dry run should list the clean tree: {dry_stdout}"
    );
    assert!(
        !dry_stdout.contains("gc ahead"),
        "dry run must not list the tree with unpushed commits: {dry_stdout}"
    );
    assert!(clean_path.exists(), "--dry-run must not touch anything");
    assert!(ahead_path.exists());

    let real = run_wt(&root, &["gc"]);
    assert_success(&real, "gc");
    assert!(!clean_path.exists(), "gc should have reaped the clean tree");
    assert!(
        ahead_path.exists(),
        "gc must leave the tree with unpushed commits alone"
    );
    assert!(
        !git_branch_exists(&base, "josh/gc-clean"),
        "gc should delete the reaped tree's branch"
    );

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    let names: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["gc ahead"]);

    assert_success(
        &run_wt(&root, &["rm", "gc ahead", "--force", "--delete-branch"]),
        "cleanup",
    );
}

#[test]
fn status_and_wait_track_background_provisioning_through_a_real_step() {
    let tmp = unique_dir("status-wait");
    let base = fixture_repo_with_submodule(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let mut cmd = Command::new(wt_bin());
    cmd.args(["new", "myrepo", "--name", "provisioned tree"])
        .env("WT_ROOT", &root)
        .env("GIT_ALLOW_PROTOCOL", "file");
    let out = cmd.output().expect("spawn wt new");
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );

    // Immediately after `new` returns, the submodule step should still be
    // registered as at least queued — proving provisioning truly runs after
    // the parent has already returned the path, not before.
    let status = run_wt(&root, &["status", "--json"]);
    assert_success(&status, "status --json");
    let entries: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let entries = entries.as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "status with no selector should list the one non-ready tree"
    );
    assert_eq!(entries[0]["name"], "provisioned tree");
    assert_ne!(
        entries[0]["state"], "ready",
        "provisioning just started; it should not be ready yet"
    );

    let wait = run_wt(&root, &["wait", "provisioned tree", "--timeout", "60"]);
    assert_success(&wait, "wait");
    assert!(
        tree_path.join("vendor/sub/f.txt").exists(),
        "the submodule step should have completed"
    );

    let status_after = run_wt(&root, &["status", "--json"]);
    assert_success(&status_after, "status --json after wait");
    let entries: serde_json::Value = serde_json::from_slice(&status_after.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        0,
        "a ready tree should not show up with no selector"
    );

    let log_path = tree_path.join(".wt-provision.log");
    assert!(
        log_path.exists(),
        "the provisioning log should exist at the path status/wait report"
    );

    assert_success(
        &run_wt(&root, &["rm", "provisioned tree", "--delete-branch"]),
        "cleanup",
    );
}

#[test]
fn doctor_reports_stale_and_unregistered_entries_and_fix_prunes_stale_ones() {
    let tmp = unique_dir("doctor");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let out = run_wt(&root, &["new", "myrepo", "--name", "will go stale"]);
    assert_success(&out, "new");
    let stale_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "will go stale"]), "wait");

    // A worktree git knows about that wt never created.
    let unregistered_path = tmp.join("manual-worktree");
    git(
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "manual-branch",
            unregistered_path.to_str().unwrap(),
            "origin/master",
        ],
        &base,
    );

    // Force the registered tree into drift without going through `wt rm`.
    std::fs::remove_dir_all(&stale_path).unwrap();

    let report = run_wt(&root, &["doctor"]);
    assert_success(&report, "doctor");
    let report_stdout = String::from_utf8_lossy(&report.stdout);
    assert!(
        report_stdout.contains("stale registry entry"),
        "expected a stale-entry line:\n{report_stdout}"
    );
    assert!(
        report_stdout.contains("will go stale"),
        "stale report should name the tree:\n{report_stdout}"
    );
    assert!(
        report_stdout.contains("unregistered worktree"),
        "expected an unregistered-worktree line:\n{report_stdout}"
    );
    assert!(
        report_stdout.contains("manual-worktree"),
        "unregistered report should name the stray path:\n{report_stdout}"
    );

    let fixed = run_wt(&root, &["doctor", "--fix"]);
    assert_success(&fixed, "doctor --fix");

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        0,
        "--fix should drop the stale registry entry"
    );
    assert!(
        unregistered_path.exists(),
        "doctor must never touch a worktree it doesn't own"
    );

    git(
        &[
            "worktree",
            "remove",
            "--force",
            unregistered_path.to_str().unwrap(),
        ],
        &base,
    );
    git(&["branch", "-D", "manual-branch"], &base);
}
