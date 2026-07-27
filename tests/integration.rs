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
