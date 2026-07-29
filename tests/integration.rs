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

/// helm and toy-apps have no `.worktreeinclude` at all, and their
/// `.gitignore`s never mention `plans/` either — so `plans` only becomes
/// invisible to git through `info/exclude`, never through a repo-authored
/// ignore pattern. This proves the default-shared path (no manifest) is
/// just as invisible to `git status` as the manifest-driven path above.
#[test]
fn plans_default_share_stays_invisible_to_git_status_with_no_manifest_and_no_gitignore_entry() {
    let tmp = unique_dir("no-manifest-plans");
    let bare = tmp.join("origin.git");
    let work = tmp.join("work");
    git(&["init", "--bare", "--quiet", bare.to_str().unwrap()], &tmp);
    git(&["symbolic-ref", "HEAD", "refs/heads/master"], &bare);
    git(
        &["init", "--quiet", "-b", "master", work.to_str().unwrap()],
        &tmp,
    );

    std::fs::write(work.join("README.md"), "fixture\n").unwrap();
    // A .gitignore exists, like helm's and toy-apps' do, but it never
    // mentions `plans` — nothing here covers the symlink `wt` will add.
    std::fs::write(work.join(".gitignore"), "*.log\n").unwrap();

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
        "base git status not clean after init with no manifest:\n{base_status}"
    );
    assert!(
        std::fs::symlink_metadata(work.join("plans"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "base should still get a plans symlink with no .worktreeinclude"
    );

    let out = run_wt(&root, &["new", "myrepo", "--name", "no manifest check"]);
    assert_success(&out, "new");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tree_path = PathBuf::from(stdout.lines().last().unwrap().trim());
    assert_success(&run_wt(&root, &["wait", "no manifest check"]), "wait");

    let tree_status = git_status_porcelain(&tree_path);
    assert!(
        tree_status.is_empty(),
        "tree git status not clean after new with no manifest:\n{tree_status}"
    );
    assert!(
        std::fs::symlink_metadata(tree_path.join("plans"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the tree should get a plans symlink by default with no .worktreeinclude"
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
fn launch_on_an_unknown_repo_fails_before_touching_claude() {
    let tmp = unique_dir("launch-unknown");
    let root = tmp.join("wt-root");

    let out = run_wt(&root, &["launch", "foo", "bogus-repo"]);
    assert!(!out.status.success(), "expected launch to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown repo"),
        "expected an unknown-repo error, got: {stderr}"
    );
}

#[test]
fn launch_without_repo_and_no_match_errors_without_creating_anything() {
    let tmp = unique_dir("launch-no-match");
    let root = tmp.join("wt-root");

    let out = run_wt(&root, &["launch", "ghost-name"]);
    assert!(!out.status.success(), "expected launch to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no tree named"),
        "expected a no-match error, got: {stderr}"
    );

    let out = run_wt(&root, &["ls", "--json"]);
    assert_success(&out, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        0,
        "no tree should be registered after a no-match launch"
    );
}

#[test]
fn launch_ambiguous_name_across_repos_names_both_candidates() {
    let tmp = unique_dir("launch-ambiguous");
    let root = tmp.join("wt-root");

    let dir_a = tmp.join("repo-a-src");
    std::fs::create_dir_all(&dir_a).unwrap();
    let base_a = fixture_repo(&dir_a);
    let dir_b = tmp.join("repo-b-src");
    std::fs::create_dir_all(&dir_b).unwrap();
    let base_b = fixture_repo(&dir_b);

    assert_success(
        &run_wt(
            &root,
            &["init", "repo-a", "--adopt", base_a.to_str().unwrap()],
        ),
        "init repo-a",
    );
    assert_success(
        &run_wt(
            &root,
            &["init", "repo-b", "--adopt", base_b.to_str().unwrap()],
        ),
        "init repo-b",
    );
    assert_success(
        &run_wt(&root, &["new", "repo-a", "--name", "same name"]),
        "new in repo-a",
    );
    assert_success(
        &run_wt(&root, &["new", "repo-b", "--name", "same name"]),
        "new in repo-b",
    );

    let out = run_wt(&root, &["launch", "same name"]);
    assert!(!out.status.success(), "expected launch to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("repo-a") && stderr.contains("repo-b"),
        "expected both candidates named, got: {stderr}"
    );
}

#[test]
fn launch_scratch_session_with_unknown_repo_errors() {
    let tmp = unique_dir("launch-scratch-unknown");
    let root = tmp.join("wt-root");

    let out = run_wt(&root, &["launch", "@poking-around", "bogus-repo"]);
    assert!(!out.status.success(), "expected launch to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown repo"),
        "expected an unknown-repo error, got: {stderr}"
    );
}

#[test]
fn launch_scratch_session_with_branch_errors() {
    let tmp = unique_dir("launch-scratch-branch");
    let root = tmp.join("wt-root");

    let out = run_wt(
        &root,
        &[
            "launch",
            "@poking-around",
            "some-repo",
            "--branch",
            "josh/x",
        ],
    );
    assert!(!out.status.success(), "expected launch to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--branch"),
        "expected a --branch error, got: {stderr}"
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

/// `git submodule deinit` rewrites `submodule.<name>.url`/`.active` in the
/// *common* `.git/config` — shared by base and every other worktree — so a
/// tree teardown must never call it. This asserts the exact submodule
/// config and status in base are byte-identical before and after `wt rm`,
/// not just that the tree itself is gone.
#[test]
fn rm_never_touches_shared_submodule_config() {
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
        "expected a plain 'git worktree remove' to refuse a tree with a checked-out submodule \
         — this is exactly the refusal wt must route around without calling submodule deinit"
    );

    let config_before = submodule_config(&base);
    let status_before = submodule_status(&base);

    assert_success(&run_wt(&root, &["rm", "submodule tree"]), "rm");
    assert!(
        !tree_path.exists(),
        "tree still on disk after 'wt rm': {}",
        tree_path.display()
    );

    let worktrees = Command::new("git")
        .args(["worktree", "list"])
        .current_dir(&base)
        .output()
        .expect("spawn git worktree list");
    assert!(
        !String::from_utf8_lossy(&worktrees.stdout).contains(tree_path.to_str().unwrap()),
        "git worktree list still mentions the removed tree"
    );

    assert_eq!(
        submodule_config(&base),
        config_before,
        "wt rm must never rewrite the shared submodule config"
    );
    assert_eq!(
        submodule_status(&base),
        status_before,
        "wt rm must never change base's own submodule status"
    );

    let out = run_wt(&root, &["ls", "--json"]);
    assert_success(&out, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 0);
}

fn submodule_config(base: &Path) -> String {
    let out = Command::new("git")
        .args(["config", "--get-regexp", "^submodule\\."])
        .current_dir(base)
        .output()
        .expect("spawn git config --get-regexp");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn submodule_status(base: &Path) -> String {
    let out = Command::new("git")
        .args(["submodule", "status"])
        .current_dir(base)
        .output()
        .expect("spawn git submodule status");
    String::from_utf8_lossy(&out.stdout).to_string()
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

fn git_rev_parse(path: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(path)
        .output()
        .expect("spawn git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Pushes one new commit straight to `bare` from a throwaway clone, so a
/// test can put `origin/<trunk>` ahead of `base` without touching `base`
/// itself.
fn push_new_commit_to_origin(bare: &Path, tmp: &Path, filename: &str) {
    let clone_dir = tmp.join(format!("advance-{}", Uuid::now_v7()));
    git(
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ],
        tmp,
    );
    std::fs::write(clone_dir.join(filename), "advance\n").unwrap();
    git(&["add", "-A"], &clone_dir);
    git(
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "advance",
        ],
        &clone_dir,
    );
    git(&["push", "-q", "origin", "master"], &clone_dir);
}

#[test]
fn sync_fast_forwards_a_clean_base_on_trunk() {
    let tmp = unique_dir("sync-ff");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    push_new_commit_to_origin(&tmp.join("origin.git"), &tmp, "advance.txt");
    let head_before = git_rev_parse(&base, "HEAD");

    let out = run_wt(&root, &["sync", "myrepo"]);
    assert_success(&out, "sync");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("fast-forwarded"),
        "expected a fast-forward line: {stdout}"
    );

    assert_ne!(
        head_before,
        git_rev_parse(&base, "HEAD"),
        "base HEAD should have moved"
    );
    assert!(
        base.join("advance.txt").exists(),
        "fast-forward should update base's working tree"
    );
}

#[test]
fn sync_refuses_and_touches_nothing_when_base_is_dirty() {
    let tmp = unique_dir("sync-dirty");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    push_new_commit_to_origin(&tmp.join("origin.git"), &tmp, "advance.txt");
    std::fs::write(base.join("dirty.txt"), "uncommitted\n").unwrap();
    let head_before = git_rev_parse(&base, "HEAD");

    let out = run_wt(&root, &["sync", "myrepo"]);
    assert!(!out.status.success(), "sync should fail when base is dirty");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("dirty"),
        "expected a dirty message: {stdout}"
    );

    assert_eq!(
        head_before,
        git_rev_parse(&base, "HEAD"),
        "a dirty base must not be fast-forwarded"
    );
    assert!(
        base.join("dirty.txt").exists(),
        "the dirty file itself must be left alone"
    );
}

#[test]
fn sync_skips_fast_forward_when_base_is_on_another_branch() {
    let tmp = unique_dir("sync-branch");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    git(&["checkout", "-q", "-b", "not-trunk"], &base);

    let out = run_wt(&root, &["sync", "myrepo"]);
    assert_success(&out, "sync");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("skipping fast-forward"),
        "expected a skip message: {stdout}"
    );
}

/// Runs `wt claude` with `PATH` pointed at an empty directory, so resolution
/// always reaches the same "claude is not on PATH" failure regardless of
/// whether the host actually has `claude` installed — the assertions below
/// only care what happened *before* that point.
fn run_wt_claude_without_claude_on_path(root: &Path, args: &[&str]) -> Output {
    Command::new(wt_bin())
        .args(args)
        .env("WT_ROOT", root)
        .env("PATH", "/nonexistent-bin-dir")
        .output()
        .expect("spawn wt")
}

#[test]
fn claude_on_a_bare_repo_name_warns_about_base_then_fails_on_missing_binary() {
    let tmp = unique_dir("claude-base");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    let out = run_wt_claude_without_claude_on_path(&root, &["claude", "myrepo"]);
    assert!(
        !out.status.success(),
        "expected failure with claude off PATH"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("base is for reading"),
        "missing base notice: {stderr}"
    );
    assert!(
        stderr.contains("not on PATH"),
        "missing not-on-PATH error: {stderr}"
    );
}

#[test]
fn claude_on_a_tree_selector_skips_the_base_notice() {
    let tmp = unique_dir("claude-tree");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    assert_success(
        &run_wt(&root, &["new", "myrepo", "--name", "claude target"]),
        "new",
    );
    assert_success(&run_wt(&root, &["wait", "claude target"]), "wait");

    let out = run_wt_claude_without_claude_on_path(&root, &["claude", "claude target"]);
    assert!(
        !out.status.success(),
        "expected failure with claude off PATH"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("base is for reading"),
        "a tree target should not print the base notice: {stderr}"
    );
    assert!(
        stderr.contains("not on PATH"),
        "missing not-on-PATH error: {stderr}"
    );
}

#[test]
fn claude_on_an_unknown_selector_fails_before_touching_path_lookup() {
    let tmp = unique_dir("claude-unknown");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    let out = run_wt_claude_without_claude_on_path(&root, &["claude", "nope"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no tree matches selector"),
        "expected a selector-resolution error, not a PATH error: {stderr}"
    );
}

#[test]
fn init_blocks_commits_in_base_but_not_in_a_tree() {
    let tmp = unique_dir("commit-block");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    std::fs::write(base.join("blocked.txt"), "nope\n").unwrap();
    git(&["add", "-A"], &base);
    let commit = Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "should be blocked",
        ])
        .current_dir(&base)
        .output()
        .expect("spawn git commit");
    assert!(
        !commit.status.success(),
        "a commit in base should be blocked"
    );
    assert!(
        String::from_utf8_lossy(&commit.stderr).contains("wt new"),
        "the block message should point at `wt new`: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    let out = run_wt(&root, &["new", "myrepo", "--name", "commit ok here"]);
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "commit ok here"]), "wait");

    std::fs::write(tree_path.join("allowed.txt"), "yes\n").unwrap();
    git(&["add", "-A"], &tree_path);
    let commit = Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "should succeed",
        ])
        .current_dir(&tree_path)
        .output()
        .expect("spawn git commit");
    assert!(
        commit.status.success(),
        "a commit in a tree must still succeed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&commit.stdout),
        String::from_utf8_lossy(&commit.stderr)
    );
}

#[test]
fn init_is_idempotent_about_the_commit_block() {
    let tmp = unique_dir("commit-block-idem");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    let args = ["init", "myrepo", "--adopt", base.to_str().unwrap()];

    assert_success(&run_wt(&root, &args), "first init");
    assert_success(&run_wt(&root, &args), "second init");

    let hooks_path_after = Command::new("git")
        .args(["config", "--worktree", "--get", "core.hooksPath"])
        .current_dir(&base)
        .output()
        .expect("spawn git config");
    assert!(hooks_path_after.status.success());

    std::fs::write(base.join("blocked-again.txt"), "nope\n").unwrap();
    git(&["add", "-A"], &base);
    let commit = Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "still blocked",
        ])
        .current_dir(&base)
        .output()
        .expect("spawn git commit");
    assert!(
        !commit.status.success(),
        "the block must survive re-running init"
    );
}

/// Appends a step to a registered repo's config directly in `data.json` —
/// there's no CLI surface for authoring a repo's provisioning steps, and a
/// test that needs a tree to stay `provisioning` for a controlled window
/// needs a step slower than anything auto-detected from a bare fixture repo.
fn inject_step(root: &Path, repo_name: &str, label: &str, cmd: &[&str]) {
    let data_path = root.join("data.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&data_path).unwrap()).unwrap();
    value["repos"][repo_name]["steps"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "label": label,
            "profile": "test",
            "cwd": ".",
            "cmd": cmd,
        }));
    std::fs::write(&data_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn mutate_tree_json(root: &Path, tree_name: &str, f: impl FnOnce(&mut serde_json::Value)) {
    let data_path = root.join("data.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&data_path).unwrap()).unwrap();
    let tree = value["trees"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["name"] == tree_name)
        .expect("tree not found in data.json");
    f(tree);
    std::fs::write(&data_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn pgrep_matches(needle: &str) -> bool {
    Command::new("pgrep")
        .args(["-f", needle])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn rm_refuses_a_provisioning_tree_without_force_then_stops_it_with_force() {
    let tmp = unique_dir("rm-provisioning");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    let marker = format!("wt-test-spin-{}", Uuid::now_v7());
    inject_step(
        &root,
        "myrepo",
        "spin",
        &[
            "sh",
            "-c",
            &format!("echo {marker}; while true; do sleep 0.2; done"),
        ],
    );

    let out = run_wt(&root, &["new", "myrepo", "--name", "spinning tree"]);
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );

    // Give the detached child a moment to actually exec the spin step.
    for _ in 0..50 {
        if pgrep_matches(&marker) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        pgrep_matches(&marker),
        "the spin step should be running before either rm attempt"
    );

    let status = run_wt(&root, &["status", "--json"]);
    assert_success(&status, "status --json");
    let entries: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(entries[0]["name"], "spinning tree");
    assert_eq!(entries[0]["state"], "provisioning");

    let refused = run_wt(&root, &["rm", "spinning tree"]);
    assert!(
        !refused.status.success(),
        "rm without --force must refuse a provisioning tree"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("provisioning") && stderr.contains("--force"),
        "expected a provisioning refusal message: {stderr}"
    );
    assert!(tree_path.exists(), "tree must survive a refused rm");
    assert!(
        pgrep_matches(&marker),
        "the spin step must still be running after a refused rm"
    );

    assert_success(
        &run_wt(&root, &["rm", "spinning tree", "--force"]),
        "rm --force",
    );
    assert!(!tree_path.exists(), "tree should be gone after --force");

    for _ in 0..20 {
        if !pgrep_matches(&marker) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        !pgrep_matches(&marker),
        "the spin step should be stopped by rm --force"
    );
}

#[test]
fn rm_force_removes_a_tree_whose_recorded_pid_is_long_gone() {
    let tmp = unique_dir("rm-dead-pid");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let out = run_wt(&root, &["new", "myrepo", "--name", "dead pid tree"]);
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "dead pid tree"]), "wait");

    // Simulate a killed-and-reaped child: state forced back to
    // `provisioning` with a pid outside any real range, so the `rm --force`
    // path has to fall through its "already gone" case rather than one it
    // can actually verify and signal.
    mutate_tree_json(&root, "dead pid tree", |t| {
        t["state"] = serde_json::json!("provisioning");
        t["provisionPid"] = serde_json::json!(999_999);
    });

    let out = run_wt(&root, &["rm", "dead pid tree", "--force"]);
    assert_success(&out, "rm --force on a tree with a dead recorded pid");
    assert!(!tree_path.exists());
}

#[test]
fn status_flags_a_provisioning_tree_whose_recorded_pid_is_dead() {
    let tmp = unique_dir("status-stale");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );
    let out = run_wt(&root, &["new", "myrepo", "--name", "wedged tree"]);
    assert_success(&out, "new");
    assert_success(&run_wt(&root, &["wait", "wedged tree"]), "wait");

    // A provisioning row with a pid that no longer resolves to anything is
    // indistinguishable from real progress unless `status` says otherwise.
    mutate_tree_json(&root, "wedged tree", |t| {
        t["state"] = serde_json::json!("provisioning");
        t["provisionPid"] = serde_json::json!(999_999);
    });

    let json_status = run_wt(&root, &["status", "--json"]);
    assert_success(&json_status, "status --json");
    let entries: serde_json::Value = serde_json::from_slice(&json_status.stdout).unwrap();
    assert_eq!(entries[0]["stale"], true);

    let text_status = run_wt(&root, &["status"]);
    assert_success(&text_status, "status");
    let stdout = String::from_utf8_lossy(&text_status.stdout);
    assert!(
        stdout.contains("stale"),
        "expected a stale marker in status output: {stdout}"
    );

    assert_success(&run_wt(&root, &["rm", "wedged tree", "--force"]), "cleanup");
}

fn git_stash_list(path: &Path) -> String {
    let out = Command::new("git")
        .args(["stash", "list"])
        .current_dir(path)
        .output()
        .expect("spawn git stash list");
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn adopt_moves_tracked_edits_into_a_fresh_tree() {
    let tmp = unique_dir("adopt-tracked");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    std::fs::write(base.join("README.md"), "edited in base by mistake\n").unwrap();

    let out = run_wt(&root, &["adopt", "myrepo", "--name", "adopt tracked"]);
    assert_success(&out, "adopt");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );

    assert!(
        git_status_porcelain(&base).is_empty(),
        "base should be clean after adopt moved the edit out"
    );
    assert_eq!(
        std::fs::read_to_string(tree_path.join("README.md")).unwrap(),
        "edited in base by mistake\n",
        "the tracked edit should have landed in the new tree"
    );
    assert!(
        git_status_porcelain(&tree_path).contains("README.md"),
        "the tree should see the edit as an uncommitted change"
    );
    assert!(
        git_stash_list(&base).is_empty(),
        "a clean pop should have dropped the stash"
    );

    assert_success(&run_wt(&root, &["wait", "adopt tracked"]), "wait");
    assert_success(
        &run_wt(
            &root,
            &["rm", "adopt tracked", "--force", "--delete-branch"],
        ),
        "cleanup",
    );
}

#[test]
fn adopt_moves_untracked_files_into_a_fresh_tree() {
    let tmp = unique_dir("adopt-untracked");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    std::fs::write(base.join("scratch.txt"), "started this in base\n").unwrap();

    let out = run_wt(&root, &["adopt", "myrepo", "--name", "adopt untracked"]);
    assert_success(&out, "adopt");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );

    assert!(
        !base.join("scratch.txt").exists(),
        "the untracked file should have moved out of base"
    );
    assert!(
        git_status_porcelain(&base).is_empty(),
        "base should be clean after adopt"
    );
    assert_eq!(
        std::fs::read_to_string(tree_path.join("scratch.txt")).unwrap(),
        "started this in base\n"
    );

    assert_success(&run_wt(&root, &["wait", "adopt untracked"]), "wait");
    assert_success(
        &run_wt(
            &root,
            &["rm", "adopt untracked", "--force", "--delete-branch"],
        ),
        "cleanup",
    );
}

#[test]
fn adopt_refuses_on_a_clean_base() {
    let tmp = unique_dir("adopt-clean");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    let out = run_wt(&root, &["adopt", "myrepo", "--name", "should fail"]);
    assert!(!out.status.success(), "adopt should refuse a clean base");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("clean") && stderr.contains("nothing to adopt"),
        "expected a clean-base refusal message: {stderr}"
    );

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        0,
        "no tree should be created when adopt refuses"
    );
}

/// Engineers a real merge conflict: base is dirtied against its current
/// commit, then a *different* commit touching the same file lands on
/// origin before the tree is created — so the stash's parent commit and the
/// tree's starting commit disagree on `file.txt`, and popping the stash
/// onto the tree can't apply cleanly.
#[test]
fn adopt_pop_conflict_leaves_the_stash_intact() {
    let tmp = unique_dir("adopt-conflict");
    let bare = tmp.join("origin.git");
    let base = tmp.join("work");
    git(&["init", "--bare", "--quiet", bare.to_str().unwrap()], &tmp);
    git(&["symbolic-ref", "HEAD", "refs/heads/master"], &bare);
    git(
        &["init", "--quiet", "-b", "master", base.to_str().unwrap()],
        &tmp,
    );
    std::fs::write(base.join("file.txt"), "original\n").unwrap();
    git(&["add", "-A"], &base);
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
        &base,
    );
    git(&["remote", "add", "origin", bare.to_str().unwrap()], &base);
    git(&["push", "-q", "origin", "master"], &base);
    git(&["fetch", "-q", "origin"], &base);
    git(&["remote", "set-head", "origin", "-a"], &base);

    let root = tmp.join("wt-root");
    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    // Dirty base with an edit that will conflict with a commit that lands
    // on origin between the stash and the new tree's checkout.
    std::fs::write(base.join("file.txt"), "conflict edit\n").unwrap();
    push_new_commit_to_origin(&bare, &tmp, "file.txt");

    let out = run_wt(&root, &["adopt", "myrepo", "--name", "conflict adopt"]);
    assert!(
        !out.status.success(),
        "adopt should fail when the pop conflicts"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("stash") && stderr.contains("intact"),
        "expected the stash-intact message: {stderr}"
    );
    assert!(
        stderr.contains("stash pop"),
        "expected recovery instructions: {stderr}"
    );

    assert!(
        git_status_porcelain(&base).is_empty(),
        "base's dirty edit should still be stashed, not left behind"
    );
    assert_eq!(
        git_stash_list(&base).lines().count(),
        1,
        "the stash must still be intact after a failed pop"
    );

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    let entries = entries.as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "the half-adopted tree should stay registered for the user to resolve"
    );
    assert_eq!(entries[0]["state"], "failed");
    let tree_path = PathBuf::from(entries[0]["path"].as_str().unwrap());
    assert!(
        tree_path.exists(),
        "the tree must survive a failed pop so its conflict can be resolved by hand"
    );

    git(&["stash", "drop"], &base);
    std::fs::remove_dir_all(&tree_path).ok();
    Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(&base)
        .status()
        .unwrap();
    run_wt(
        &root,
        &["rm", "conflict adopt", "--force", "--delete-branch"],
    );
}

#[test]
fn env_refresh_recopies_and_overwrites_a_stale_env_file() {
    let tmp = unique_dir("env-refresh");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    std::fs::write(base.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(base.join(".env"), "A=1\n").unwrap();
    git(&["add", "-A"], &base);
    git(
        &[
            "-c",
            "user.email=wt-cli-test@example.com",
            "-c",
            "user.name=wt-cli-test",
            "commit",
            "-q",
            "-m",
            "add gitignored env",
        ],
        &base,
    );
    git(&["push", "-q", "origin", "master"], &base);
    git(&["fetch", "-q", "origin"], &base);

    assert_success(
        &run_wt(
            &root,
            &["init", "myrepo", "--adopt", base.to_str().unwrap()],
        ),
        "init",
    );

    let out = run_wt(&root, &["new", "myrepo", "--name", "env refresh target"]);
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "env refresh target"]), "wait");

    assert_eq!(
        std::fs::read_to_string(tree_path.join(".env")).unwrap(),
        "A=1\n"
    );

    // base's env changes after the tree already has a now-stale copy.
    std::fs::write(base.join(".env"), "A=2\n").unwrap();

    let refresh = run_wt(&root, &["env", "refresh", "env refresh target"]);
    assert_success(&refresh, "env refresh");
    let stdout = String::from_utf8_lossy(&refresh.stdout);
    assert!(
        stdout.contains(".env"),
        "expected the copied file to be reported: {stdout}"
    );

    assert_eq!(
        std::fs::read_to_string(tree_path.join(".env")).unwrap(),
        "A=2\n",
        "env refresh should overwrite the tree's stale copy"
    );

    assert_success(
        &run_wt(&root, &["rm", "env refresh target", "--delete-branch"]),
        "cleanup",
    );
}
