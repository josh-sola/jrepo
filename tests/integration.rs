//! End-to-end coverage against throwaway git repos under the OS temp dir.
//! Nothing here touches a real clone — every fixture is created fresh and
//! `WT_ROOT` is passed per-process so parallel tests never collide.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use uuid::Uuid;

fn wt_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_wt"))
}

/// Drops this process, and everything it spawns, to background quality of
/// service.
///
/// The suite competes with the desktop for CPU and, far more painfully, for
/// disk: it builds dozens of git repos and checks out worktrees while the
/// machine is in use. Background QoS throttles both, and children inherit
/// it, so the run gets slower but never makes the machine stutter. Capping
/// the thread count alone does not achieve this — 4 test threads still fan
/// out into far more git processes, all competing at normal priority.
///
/// Best effort: an older or non-macOS host simply runs unthrottled.
fn deprioritize_this_run() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = Command::new("/usr/sbin/taskpolicy")
            .args(["-b", "-p", &std::process::id().to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// A test's private directory, holding both its git fixtures and its
/// `WT_ROOT`. Dropping it stops whatever the test left running and deletes
/// the tree.
///
/// Derefs to `Path`, so it is used exactly like the `PathBuf` it replaced.
struct TestDir(PathBuf);

impl std::ops::Deref for TestDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        stop_detached_children(&self.0);
        // A failing test's directory is the only evidence of why, so it
        // survives; the processes never do.
        if std::thread::panicking() {
            eprintln!("test failed — leaving {} in place", self.0.display());
            return;
        }
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// Every test starts by claiming its directory, which makes this the one
/// place guaranteed to run before any test does work.
fn unique_dir(label: &str) -> TestDir {
    deprioritize_this_run();
    let dir = std::env::temp_dir().join(format!("wt-cli-it-{label}-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    TestDir(dir)
}

/// Kills the provisioning children `wt` spawned under `dir`.
///
/// Those children are put in their own process group on purpose, so a
/// signal to the test runner never reaches them and they outlive the run.
/// The registry records each one's pid, which is the only handle a test has
/// on them.
fn stop_detached_children(dir: &Path) {
    for registry in registries_under(dir) {
        let Ok(text) = std::fs::read_to_string(&registry) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(trees) = value["trees"].as_array() else {
            continue;
        };
        for pid in trees.iter().filter_map(|t| t["provisionPid"].as_u64()) {
            kill_if_ours(pid);
        }
    }
}

/// A recorded pid may belong to a child that already exited, and the number
/// can be reused by anything, so the command line is checked before
/// signalling. The negative pid reaches the whole group, which is where a
/// step's own children (an install, say) live.
fn kill_if_ours(pid: u64) {
    let Ok(out) = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
    else {
        return;
    };
    if !String::from_utf8_lossy(&out.stdout).contains("wt") {
        return;
    }
    let _ = Command::new("kill")
        .args(["-9", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// A test names its `WT_ROOT` whatever it likes inside its own directory,
/// so the registry is found rather than assumed.
fn registries_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let direct = dir.join("state.json");
    if direct.exists() {
        found.push(direct);
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("state.json");
            if candidate.exists() {
                found.push(candidate);
            }
        }
    }
    found
}

/// Config every git process in the suite runs under, in place of the
/// developer's own.
///
/// `gc.auto`, `maintenance.auto`, and `core.fsmonitor` each spawn detached
/// background processes that outlive the test that created the repo. The
/// suite builds dozens of throwaway repos per run, so leaving those on
/// leaves a pile of daemons churning on the machine long after the run
/// finishes — and nothing in the run ever reaps them.
fn hermetic_git_config() -> &'static Path {
    static CONFIG: OnceLock<PathBuf> = OnceLock::new();
    CONFIG.get_or_init(|| {
        // A fixed name, not a per-run one: the contents never vary, so
        // concurrent runs rewriting identical bytes is harmless, and the
        // suite leaves one small file behind instead of one per run.
        // Stock `git init` writes sixteen sample hooks nobody runs, turning
        // an 2-file repo into an 18-file one. Multiplied across the repos a
        // run creates, that is the bulk of its filesystem traffic — and on a
        // machine with endpoint security software, every one of those files
        // is inspected as it appears.
        let empty_template = std::env::temp_dir().join("wt-cli-it-empty-template");
        std::fs::create_dir_all(&empty_template).expect("create empty git template");

        let path = std::env::temp_dir().join("wt-cli-it-gitconfig");
        std::fs::write(
            &path,
            format!(
                "[gc]\n\tauto = 0\n\
                 [maintenance]\n\tauto = false\n\
                 [core]\n\tfsmonitor = false\n\
                 [protocol]\n\tversion = 2\n\
                 [user]\n\temail = wt-cli-test@example.com\n\tname = wt-cli-test\n\
                 [commit]\n\tgpgsign = false\n\
                 [init]\n\ttemplateDir = {}\n",
                empty_template.display()
            ),
        )
        .expect("write test gitconfig");
        path
    })
}

fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", hermetic_git_config())
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
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
///
/// Copied from a template built once per run rather than built from
/// scratch, which trades ten git processes per test for one clone. On APFS
/// the copy is copy-on-write, so it costs metadata rather than data.
fn fixture_repo(dir: &Path) -> PathBuf {
    let template = fixture_template();
    let bare = dir.join("origin.git");
    let work = dir.join("work");
    clone_tree(&template.join("origin.git"), &bare);
    clone_tree(&template.join("work"), &work);
    // The copy still names the template's bare repo as its origin.
    git(
        &["remote", "set-url", "origin", bare.to_str().unwrap()],
        &work,
    );
    work
}

/// Copies a directory using APFS cloning, so the bytes are shared until one
/// side writes.
fn clone_tree(src: &Path, dst: &Path) {
    let out = Command::new("cp")
        .args(["-Rc", &src.to_string_lossy(), &dst.to_string_lossy()])
        .output()
        .expect("spawn cp");
    assert!(
        out.status.success(),
        "cp -Rc {} {} failed: {}",
        src.display(),
        dst.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The pristine fixture every test copies, built at most once per run and
/// rebuilt if the cached copy no longer checks out.
///
/// Staged under a unique name and renamed into place so two runs racing
/// here cannot observe a half-built template; the loser discards its own
/// copy and uses the winner's.
fn fixture_template() -> &'static Path {
    static TEMPLATE: OnceLock<PathBuf> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| ensure_fixture_template(&std::env::temp_dir().join("wt-cli-it-template")))
}

/// Returns `final_path` holding a usable template, rebuilding it there if it
/// is missing or broken.
///
/// The OS can prune files out of a long-idle temp cache while leaving its
/// directories in place, so a directory-existence check cannot prove the
/// template usable; only a real git read on it can.
fn ensure_fixture_template(final_path: &Path) -> PathBuf {
    if template_is_valid(final_path) {
        return final_path.to_path_buf();
    }
    let parent = final_path
        .parent()
        .expect("template path must have a parent");
    let name = final_path
        .file_name()
        .expect("template path must have a name");
    // Moved aside under a unique name and deleted from there, not in place:
    // a concurrent run mid-`cp` from the template keeps its inode either way.
    let stale = parent.join(format!(
        "{}-stale-{}",
        name.to_string_lossy(),
        Uuid::now_v7()
    ));
    if std::fs::rename(final_path, &stale).is_ok() {
        std::fs::remove_dir_all(&stale).ok();
    }
    let staging = parent.join(format!("wt-cli-it-staging-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&staging).expect("create template staging dir");
    build_fixture_repo(&staging);
    if std::fs::rename(&staging, final_path).is_err() {
        std::fs::remove_dir_all(&staging).ok();
    }
    final_path.to_path_buf()
}

/// Whether both halves `fixture_repo` copies out of the template — the bare
/// `origin.git` and the `work` clone — are git repos that can actually be
/// read, not just directories that exist.
fn template_is_valid(template_path: &Path) -> bool {
    head_resolves(&template_path.join("origin.git")) && head_resolves(&template_path.join("work"))
}

fn head_resolves(repo: &Path) -> bool {
    Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "rev-parse",
            "--verify",
            "HEAD",
        ])
        .env("GIT_CONFIG_GLOBAL", hermetic_git_config())
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn build_fixture_repo(dir: &Path) -> PathBuf {
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

/// `WT_CONFIG` for a `WT_ROOT` living at `root`, so a test never reaches the
/// developer's own `~/.config/wt/config.kdl`. Every test's root is a
/// `wt-root` subdirectory of its own private tmp dir, so the config file is
/// placed next to it rather than inside it — sharing that tmp dir with
/// `state.json` without the two ever colliding.
fn config_path_for(root: &Path) -> PathBuf {
    root.parent()
        .expect("root must live under the test's tmp dir")
        .join("config.kdl")
}

fn run_wt(root: &Path, args: &[&str]) -> Output {
    Command::new(wt_bin())
        .args(args)
        .env("WT_ROOT", root)
        .env("WT_CONFIG", config_path_for(root))
        // `wt` shells out to git constantly, including from the detached
        // children it spawns, so the same quiescing has to reach those too.
        .env("GIT_CONFIG_GLOBAL", hermetic_git_config())
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
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

/// Registers `base` as `name`'s repo, then turns off hot spares. The suite
/// runs every test in parallel across every core; a background spare build
/// on top of each test's own `wt new` calls would multiply the number of
/// concurrent `git worktree add`s and installs many times over, and one
/// that inherits a step a test deliberately made spin forever would never
/// exit. Tests that mean to exercise spares opt back in with
/// `enable_spares`.
fn init_repo(root: &Path, name: &str, base: &Path) {
    assert_success(
        &run_wt(root, &["init", name, "--adopt", base.to_str().unwrap()]),
        &format!("init {name}"),
    );
    set_spares(root, name, 0);
}

/// Opts a repo back into hot spares for a test that means to exercise them.
fn enable_spares(root: &Path, repo_name: &str, n: u8) {
    set_spares(root, repo_name, n);
}

/// Rewrites `repo_name`'s `spares` line in `config.kdl`, touching only that
/// repo's block — a config with more than one `repo` block (`repo-a`,
/// `repo-b`) makes a whole-file replace wrong.
fn set_spares(root: &Path, repo_name: &str, n: u8) {
    let config_path = config_path_for(root);
    let text = std::fs::read_to_string(&config_path).unwrap();
    let body = repo_block_body(&text, repo_name);

    let marker = "spares ";
    let rel = text[body.clone()]
        .find(marker)
        .unwrap_or_else(|| panic!("no 'spares' line in {repo_name}'s config block"));
    let digits_start = body.start + rel + marker.len();
    let digits_end = digits_start
        + text[digits_start..]
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count();

    let mut new_text = String::with_capacity(text.len());
    new_text.push_str(&text[..digits_start]);
    new_text.push_str(&n.to_string());
    new_text.push_str(&text[digits_end..]);
    std::fs::write(&config_path, new_text).unwrap();
}

/// The byte range of a `repo "<name>" { ... }` block's body in `text`,
/// found by tracking brace depth from the block's own opening brace — the
/// scope every block-local edit in this file works within.
fn repo_block_body(text: &str, repo_name: &str) -> std::ops::Range<usize> {
    let needle = format!("repo \"{repo_name}\" {{");
    let open = text
        .find(&needle)
        .unwrap_or_else(|| panic!("no repo block named '{repo_name}' in config"))
        + needle.len()
        - 1;

    let bytes = text.as_bytes();
    let mut depth = 1;
    let mut i = open + 1;
    loop {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    (open + 1)..i
}

/// A spare's registered name, so a test can pick it out of `wt ls --all
/// --json` without hardcoding the string twice.
const SPARE_NAME: &str = "@spare";

/// The spare rows for `repo`, read through `wt ls --all --json` — `wt spare
/// --json` never reports a spare's path, and a claim, a corrupted registry
/// row, and a dirty working tree all need it.
fn spare_rows(root: &Path, repo: &str) -> Vec<serde_json::Value> {
    let out = run_wt(root, &["ls", "--all", "--json", "--repo", repo]);
    assert_success(&out, "ls --all --json");
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    entries
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["spare"] == true)
        .cloned()
        .collect()
}

/// Blocks until at least one of `repo`'s spares is `ready` or `failed`,
/// returning that row. A test builds a spare through `wt sync` rather than
/// `wt new`, so the claim it means to test is a separate, later step.
fn wait_for_settled_spare(root: &Path, repo: &str, timeout_secs: u64) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        let rows = spare_rows(root, repo);
        if let Some(row) = rows
            .iter()
            .find(|r| r["state"] == "ready" || r["state"] == "failed")
        {
            return row.clone();
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "timed out after {timeout_secs}s waiting for {repo}'s spare to settle: {rows:?}"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Blocks until every spare row for `repo` is `ready` or `failed` — the
/// drain a spare-enabled test runs before it returns, so a background
/// top-up build is never still running once the test's directory is torn
/// down.
fn wait_for_all_spares_settled(
    root: &Path,
    repo: &str,
    timeout_secs: u64,
) -> Vec<serde_json::Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        let rows = spare_rows(root, repo);
        if !rows.is_empty()
            && rows
                .iter()
                .all(|r| r["state"] == "ready" || r["state"] == "failed")
        {
            return rows;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "timed out after {timeout_secs}s waiting for {repo}'s spares to settle: {rows:?}"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Builds `repo`'s one hot spare from cold with `wt sync` (which fetches,
/// then tops up any shortfall) and waits for it to settle.
fn build_and_wait_spare(root: &Path, repo: &str, timeout_secs: u64) -> serde_json::Value {
    assert_success(&run_wt(root, &["sync", repo]), "sync (spare top-up)");
    wait_for_settled_spare(root, repo, timeout_secs)
}

/// Claims `repo`'s one ready spare with a real `wt new` and waits for the
/// resulting tree — the shared setup behind the top-up assertions, which
/// only differ in what they check afterward. Returns the claimed spare's
/// original id.
fn build_claim_and_wait(root: &Path, repo: &str, name: &str) -> String {
    let spare = build_and_wait_spare(root, repo, 60);
    assert_eq!(spare["state"], "ready");
    let original_id = spare["id"].as_str().unwrap().to_string();

    let out = run_wt(root, &["new", repo, "--name", name]);
    assert_success(&out, "new (claim)");
    assert_success(&run_wt(root, &["wait", name]), "wait");
    original_id
}

/// Counts the lines a provisioning step injected by `inject_step` has
/// appended to its own marker file. The marker is separate from
/// `.wt-provision.log`, which is truncated on every run and so cannot
/// distinguish one run from two.
fn marker_run_count(marker: &Path) -> usize {
    std::fs::read_to_string(marker)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .count()
}

/// Spawns `wt` without waiting for it, so two calls can race each other
/// against the same spare.
fn spawn_wt(root: &Path, args: &[&str]) -> std::process::Child {
    Command::new(wt_bin())
        .args(args)
        .env("WT_ROOT", root)
        .env("WT_CONFIG", config_path_for(root))
        .env("GIT_CONFIG_GLOBAL", hermetic_git_config())
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn wt")
}

/// Pushes a commit straight to `bare` that overwrites `README.md`, so a
/// test can put origin ahead of a spare with a change that will conflict
/// with an uncommitted edit to the same file.
fn push_readme_commit(bare: &Path, tmp: &Path, content: &str) {
    let clone_dir = tmp.join(format!("advance-readme-{}", Uuid::now_v7()));
    git(
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ],
        tmp,
    );
    std::fs::write(clone_dir.join("README.md"), content).unwrap();
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
            "advance readme",
        ],
        &clone_dir,
    );
    git(&["push", "-q", "origin", "master"], &clone_dir);
}

#[test]
fn init_new_ls_rm_round_trip() {
    let tmp = unique_dir("e2e");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &work);

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
    init_repo(&root, "myrepo", &work);

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

    init_repo(&root, "myrepo", &base);

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

    init_repo(&root, "repo-a", &base_a);
    init_repo(&root, "repo-b", &base_b);
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

/// A `get-position` hook that just creates a sentinel file — enough to prove
/// whether `wt launch` ran it, without needing a real Ghostty or a real
/// `claude` session. Uses a shell redirect rather than `touch`: these tests
/// point `PATH` at a nonexistent directory, which an external command would
/// need to resolve.
fn sentinel_get_position_hook(sentinel: &Path) -> String {
    format!(
        "version 1\n\nfeatures {{\n    planter {{\n        get-position {{ cmd \"/bin/sh\" \"-c\" \": > {}\" }}\n    }}\n}}\n",
        sentinel.display()
    )
}

#[test]
fn launch_runs_a_hook_only_when_its_feature_block_is_present() {
    let tmp = unique_dir("launch-features-gate");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    assert_success(
        &run_wt(&root, &["new", "myrepo", "--name", "hook target"]),
        "new",
    );
    assert_success(&run_wt(&root, &["wait", "hook target"]), "wait");

    let sentinel = tmp.join("sentinel");
    let config_path = config_path_for(&root);

    // No `features` block: the hook the config could have pointed at never runs.
    std::fs::write(&config_path, "version 1\n").unwrap();
    let out = run_wt_claude_without_claude_on_path(&root, &["launch", "hook target", "myrepo"]);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not on PATH"),
        "expected the launch to still reach the claude exec, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !sentinel.exists(),
        "a hook must not run when its feature block is absent"
    );

    // The same hook, declared under `features`: it runs.
    std::fs::write(&config_path, sentinel_get_position_hook(&sentinel)).unwrap();
    let out = run_wt_claude_without_claude_on_path(&root, &["launch", "hook target", "myrepo"]);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not on PATH"),
        "expected the launch to still reach the claude exec, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        sentinel.exists(),
        "a hook declared under 'features' should have run"
    );
}

#[test]
fn name_matches_longest_registered_path_prefix() {
    let tmp = unique_dir("name");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
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

    init_repo(&root, "myrepo", &base);
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

    init_repo(&root, "myrepo", &base);
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

    init_repo(&root, "myrepo", &base);
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

    init_repo(&root, "myrepo", &base);
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

    init_repo(&root, "myrepo", &base);
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
fn gc_reaps_a_tree_whose_branch_has_graphite_children_but_keeps_the_branch() {
    let tmp = unique_dir("gc-children");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    assert_success(
        &run_wt(
            &root,
            &["new", "myrepo", "--name", "tree a", "--branch", "a"],
        ),
        "new tree a",
    );
    assert_success(&run_wt(&root, &["wait", "tree a"]), "wait");
    let path_out = run_wt(&root, &["path", "tree a"]);
    assert_success(&path_out, "path");
    let tree_a_path = String::from_utf8_lossy(&path_out.stdout).trim().to_string();

    // `b` is tracked as a Graphite child of `a` but has no worktree of its
    // own — enough to prove the check, without needing a second tree.
    let db = base.join(".git").join(".graphite_metadata.db");
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
         ('master', NULL, 'TRUNK'), ('a', 'master', NULL), ('b', 'a', NULL);",
    );

    let out = run_wt(&root, &["gc"]);
    assert_success(&out, "gc");
    assert!(
        !PathBuf::from(&tree_a_path).exists(),
        "gc must still reclaim the worktree — the children only block deleting the branch"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("reaping") && stdout.contains("keeping branch 'a'"),
        "gc should say it reaped 'tree a' but kept its branch: {stdout}"
    );
    assert!(
        git_branch_exists(&base, "a"),
        "branch 'a' must survive so 'b' keeps its parent"
    );
}

#[test]
fn rm_delete_branch_refuses_a_mid_stack_branch_and_force_still_bypasses_it() {
    let tmp = unique_dir("rm-children");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    assert_success(
        &run_wt(
            &root,
            &["new", "myrepo", "--name", "tree a", "--branch", "a"],
        ),
        "new tree a",
    );
    assert_success(&run_wt(&root, &["wait", "tree a"]), "wait");

    let db = base.join(".git").join(".graphite_metadata.db");
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
         ('master', NULL, 'TRUNK'), ('a', 'master', NULL), ('b', 'a', NULL);",
    );

    let refused = run_wt(&root, &["rm", "tree a", "--delete-branch"]);
    assert!(
        !refused.status.success(),
        "must refuse a mid-stack branch without --force or --reparent-children"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains('b') && stderr.contains("--reparent-children"),
        "refusal should name the child and the escape hatch: {stderr}"
    );
    assert!(
        git_branch_exists(&base, "a"),
        "a refusal must delete nothing"
    );

    assert_success(
        &run_wt(&root, &["rm", "tree a", "--force", "--delete-branch"]),
        "rm --force --delete-branch",
    );
    assert!(
        !git_branch_exists(&base, "a"),
        "--force must still bypass the children check"
    );
}

#[test]
fn status_and_wait_track_background_provisioning_through_a_real_step() {
    let tmp = unique_dir("status-wait");
    let base = fixture_repo_with_submodule(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    let mut cmd = Command::new(wt_bin());
    cmd.args(["new", "myrepo", "--name", "provisioned tree"])
        .env("WT_ROOT", &root)
        .env("WT_CONFIG", config_path_for(&root))
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

    init_repo(&root, "myrepo", &base);
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
    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &base);

    git(&["checkout", "-q", "-b", "not-trunk"], &base);

    let out = run_wt(&root, &["sync", "myrepo"]);
    assert_success(&out, "sync");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("skipping fast-forward"),
        "expected a skip message: {stdout}"
    );
}

/// Advances `submodule_path`'s own upstream with one new commit, then bumps
/// `base_bare`'s recorded pointer to match — an upstream submodule bump
/// landing on the wire, without touching the test's own `base` checkout.
/// The scratch clone this builds through needs
/// `protocol.file.allow=always` for its own submodule population; `base`
/// itself never does, since its submodule was trusted once at `add` time.
fn push_submodule_bump_to_origin(base_bare: &Path, submodule_path: &str, tmp: &Path) {
    let clone_dir = tmp.join(format!("advance-submodule-{}", Uuid::now_v7()));
    git(
        &[
            "clone",
            "-q",
            base_bare.to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ],
        tmp,
    );
    git(
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "--recursive",
        ],
        &clone_dir,
    );

    let sub_dir = clone_dir.join(submodule_path);
    git(&["checkout", "-q", "master"], &sub_dir);
    std::fs::write(sub_dir.join("f.txt"), "bumped\n").unwrap();
    git(
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-aqm",
            "bump",
        ],
        &sub_dir,
    );
    git(&["push", "-q", "origin", "master"], &sub_dir);

    git(&["add", submodule_path], &clone_dir);
    git(
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "bump submodule pointer",
        ],
        &clone_dir,
    );
    git(&["push", "-q", "origin", "master"], &clone_dir);
}

/// A superproject that has already fast-forwarded past a submodule bump
/// while the checked-out submodule was left behind. `wt sync` must repair
/// the stale pointer and still complete its fast-forward in the same run,
/// and a follow-up sync must find nothing left to do.
#[test]
fn sync_repairs_a_stale_submodule_pointer_and_still_fast_forwards() {
    let tmp = unique_dir("sync-submodule-stuck");
    let base = fixture_repo_with_submodule(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);

    let base_bare = tmp.join("origin.git");
    push_submodule_bump_to_origin(&base_bare, "vendor/sub", &tmp);

    // Reproduce the stuck state directly, bypassing `wt sync` entirely: the
    // superproject fast-forwards past the bump, the submodule checkout does
    // not move.
    git(&["fetch", "-q", "origin"], &base);
    git(&["merge", "-q", "--ff-only", "origin/master"], &base);
    assert!(
        !status_porcelain(&base).is_empty(),
        "the stale gitlink should already show as dirty"
    );
    assert_eq!(
        std::fs::read_to_string(base.join("vendor/sub").join("f.txt")).unwrap(),
        "sub\n",
        "the submodule checkout must still be at its old commit"
    );

    push_new_commit_to_origin(&base_bare, &tmp, "advance.txt");

    let out = run_wt(&root, &["sync", "myrepo"]);
    assert_success(&out, "sync");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("repaired") && stdout.contains("submodule"),
        "expected the repair to be mentioned: {stdout}"
    );
    assert!(
        stdout.contains("fast-forwarded"),
        "expected the fast-forward to still happen: {stdout}"
    );

    assert_eq!(
        std::fs::read_to_string(base.join("vendor/sub").join("f.txt")).unwrap(),
        "bumped\n",
        "the submodule checkout should have caught up to the bump"
    );
    assert!(
        status_porcelain(&base).is_empty(),
        "base should be fully clean after the repair"
    );
    assert!(base.join("advance.txt").exists());

    let out = run_wt(&root, &["sync", "myrepo"]);
    assert_success(&out, "second sync");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("dirty") && !stdout.contains("repaired"),
        "a second sync should find nothing left to fix: {stdout}"
    );
}

fn status_porcelain(path: &Path) -> String {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .env("GIT_CONFIG_GLOBAL", hermetic_git_config())
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("spawn git status");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Runs `wt claude` with `PATH` pointed at an empty directory, so resolution
/// always reaches the same "claude is not on PATH" failure regardless of
/// whether the host actually has `claude` installed — the assertions below
/// only care what happened *before* that point.
fn run_wt_claude_without_claude_on_path(root: &Path, args: &[&str]) -> Output {
    Command::new(wt_bin())
        .args(args)
        .env("WT_ROOT", root)
        .env("WT_CONFIG", config_path_for(root))
        .env("PATH", "/nonexistent-bin-dir")
        .output()
        .expect("spawn wt")
}

#[test]
fn claude_on_a_bare_repo_name_warns_about_base_then_fails_on_missing_binary() {
    let tmp = unique_dir("claude-base");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &base);
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
    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &base);

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

#[test]
fn reinit_preserves_a_hand_edited_spares_value_byte_for_byte() {
    let tmp = unique_dir("reinit-preserve-spares");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    let args = ["init", "myrepo", "--adopt", base.to_str().unwrap()];

    assert_success(&run_wt(&root, &args), "first init");
    set_spares(&root, "myrepo", 0);
    let config_path = config_path_for(&root);
    let bytes_after_edit = std::fs::read(&config_path).unwrap();

    let out = run_wt(&root, &args);
    assert_success(&out, "second init");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("already has a config block"),
        "expected a no-op notice: {stdout}"
    );
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        bytes_after_edit,
        "re-init must not touch a block a person has hand-edited"
    );
}

#[test]
fn init_redetect_replaces_steps_but_leaves_everything_else_alone() {
    let tmp = unique_dir("init-redetect");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    set_spares(&root, "myrepo", 7);

    // The fixture has nothing `detect_steps` picks up on; adding a
    // pnpm-lock.yaml gives redetect something new to find.
    std::fs::write(base.join("pnpm-lock.yaml"), "lockfileVersion: '6'\n").unwrap();

    let out = run_wt(
        &root,
        &[
            "init",
            "myrepo",
            "--adopt",
            base.to_str().unwrap(),
            "--redetect",
        ],
    );
    assert_success(&out, "init --redetect");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("redetected steps for"),
        "expected a redetect notice: {stdout}"
    );

    let config_text = std::fs::read_to_string(config_path_for(&root)).unwrap();
    assert!(
        config_text.contains("pnpm-install"),
        "expected the newly detected step: {config_text}"
    );
    assert!(
        config_text.contains("spares 7"),
        "redetect must leave the hand-edited spares value alone: {config_text}"
    );
}

#[test]
fn init_with_explicit_branch_prefix_on_an_existing_repo_warns_it_was_ignored() {
    let tmp = unique_dir("branch-prefix-ignored");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);

    let out = run_wt(
        &root,
        &[
            "init",
            "myrepo",
            "--adopt",
            base.to_str().unwrap(),
            "--branch-prefix",
            "other/",
        ],
    );
    assert_success(&out, "second init with --branch-prefix");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("already has a config block"),
        "expected a no-op notice: {stdout}"
    );
    assert!(
        stdout.contains("--branch-prefix was ignored"),
        "expected a warning that --branch-prefix was ignored: {stdout}"
    );

    let config_text = std::fs::read_to_string(config_path_for(&root)).unwrap();
    assert!(
        config_text.contains("branch-prefix \"josh/\""),
        "the original branch-prefix must survive: {config_text}"
    );
}

#[test]
fn first_run_migrates_a_pre_split_data_json() {
    let tmp = unique_dir("migrate-smoke");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("data.json"),
        format!(
            r#"{{"version":1,"repos":{{"myrepo":{{"base":"{}","trunk":"master",
            "branchPrefix":"josh/","shared":[],"copy":[],"env":{{}},"steps":[],"spares":1}}}},
            "trees":[],"env":{{}}}}"#,
            base.display()
        ),
    )
    .unwrap();

    let out = run_wt(&root, &["ls"]);
    assert_success(&out, "ls (triggers migration)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("migrated to"),
        "expected a migration notice: {stdout}"
    );

    assert!(root.join("state.json").exists(), "state.json missing");
    let config_path = config_path_for(&root);
    assert!(config_path.exists(), "config.kdl missing");
    let config_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        config_text.contains("repo \"myrepo\""),
        "expected myrepo's block: {config_text}"
    );
    assert!(
        root.join("data.json.migrated").exists(),
        "data.json should have been renamed aside"
    );
    assert!(
        !root.join("data.json").exists(),
        "data.json should be gone after migration"
    );
}

/// Inserts a `step` node into `repo_name`'s block in `config.kdl` — there's
/// no CLI surface for authoring a repo's provisioning steps, and a test that
/// needs a tree to stay `provisioning` for a controlled window needs a step
/// slower than anything auto-detected from a bare fixture repo.
fn inject_step(root: &Path, repo_name: &str, label: &str, cmd: &[&str]) {
    let config_path = config_path_for(root);
    let text = std::fs::read_to_string(&config_path).unwrap();
    let body = repo_block_body(&text, repo_name);

    let args = cmd
        .iter()
        .map(|a| quote_kdl_string(a))
        .collect::<Vec<_>>()
        .join(" ");
    let step = format!(
        "    step {} profile=\"test\" cwd=\".\" {{\n        cmd {args}\n    }}\n",
        quote_kdl_string(label)
    );

    let mut new_text = String::with_capacity(text.len() + step.len());
    new_text.push_str(&text[..body.end]);
    new_text.push_str(&step);
    new_text.push_str(&text[body.end..]);
    std::fs::write(&config_path, new_text).unwrap();
}

/// Quotes a string the way `wt init` itself writes one into `config.kdl`,
/// so an injected step survives a value with a quote or backslash in it.
fn quote_kdl_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn mutate_tree_json(root: &Path, tree_name: &str, f: impl FnOnce(&mut serde_json::Value)) {
    let state_path = root.join("state.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    let tree = value["trees"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["name"] == tree_name)
        .expect("tree not found in state.json");
    f(tree);
    std::fs::write(&state_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
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
    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &base);
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
    init_repo(&root, "myrepo", &base);
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
    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &base);

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
    init_repo(&root, "myrepo", &base);

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

    init_repo(&root, "myrepo", &base);

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

/// `body` must stick to `/bin/sh` builtins. These tests point `PATH` at a
/// nonexistent directory so the post-pick `claude` exec fails predictably,
/// which would also break an external command inside the script.
fn write_fake_fzf(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-fzf.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn launch_with_no_worktree_and_no_trees_fails_before_spawning_the_picker() {
    let tmp = unique_dir("launch-picker-empty");
    let root = tmp.join("wt-root");
    let marker = tmp.join("fzf-ran");

    let fzf = write_fake_fzf(&tmp, &format!("touch '{}'", marker.display()));

    let out = Command::new(wt_bin())
        .args(["launch"])
        .env("WT_ROOT", &root)
        .env("WT_CONFIG", config_path_for(&root))
        .env("WT_FZF", &fzf)
        .output()
        .expect("spawn wt");

    assert!(!out.status.success(), "expected launch to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no worktrees registered"),
        "expected a no-worktrees message, got: {stderr}"
    );
    assert!(
        !marker.exists(),
        "the picker must not run when there is nothing to pick from"
    );
}

#[test]
fn launch_with_no_worktree_offers_the_cwd_repo_first_then_newest_first() {
    let tmp = unique_dir("launch-picker-order");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    assert_success(
        &run_wt(&root, &["new", "myrepo", "--name", "older tree"]),
        "new older",
    );
    assert_success(&run_wt(&root, &["wait", "older tree"]), "wait older");
    assert_success(
        &run_wt(&root, &["new", "myrepo", "--name", "newer tree"]),
        "new newer",
    );
    assert_success(&run_wt(&root, &["wait", "newer tree"]), "wait newer");

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    let entries = entries.as_array().unwrap();
    let id_of = |name: &str| -> String {
        entries
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("no entry named {name}"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let older_id = id_of("older tree");
    let newer_id = id_of("newer tree");

    let capture = tmp.join("fzf-stdin.txt");
    let script = format!(
        "i=0\n\
         while IFS= read -r line; do\n\
         printf '%s\\n' \"$line\" >> '{}'\n\
         i=$((i + 1))\n\
         if [ \"$i\" -eq 1 ]; then first=\"$line\"; fi\n\
         done\n\
         printf '%s\\n' \"$first\"",
        capture.display()
    );
    let fzf = write_fake_fzf(&tmp, &script);

    let out = Command::new(wt_bin())
        .args(["launch"])
        .env("WT_ROOT", &root)
        .env("WT_CONFIG", config_path_for(&root))
        .env("WT_FZF", &fzf)
        .env("PATH", "/nonexistent-bin-dir")
        .output()
        .expect("spawn wt");

    assert!(
        !out.status.success(),
        "expected the launch past the picker to fail on the missing claude binary"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not on PATH"),
        "expected the picked tree to reach the claude exec, got: {stderr}"
    );

    let captured = std::fs::read_to_string(&capture).unwrap();
    let captured_lines: Vec<&str> = captured.lines().collect();
    assert_eq!(
        captured_lines.len(),
        2,
        "expected both trees on the picker's stdin: {captured}"
    );
    assert!(
        captured_lines[0].starts_with(&format!("{newer_id}\t")),
        "expected the newer tree first, got: {captured}"
    );
    assert!(
        captured_lines[1].starts_with(&format!("{older_id}\t")),
        "expected the older tree second, got: {captured}"
    );
}

#[test]
fn launch_with_no_worktree_and_a_cancelled_picker_exits_cleanly() {
    let tmp = unique_dir("launch-picker-cancel");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    assert_success(
        &run_wt(&root, &["new", "myrepo", "--name", "cancel target"]),
        "new",
    );
    assert_success(&run_wt(&root, &["wait", "cancel target"]), "wait");

    let fzf = write_fake_fzf(&tmp, "exit 130");

    let out = Command::new(wt_bin())
        .args(["launch"])
        .env("WT_ROOT", &root)
        .env("WT_CONFIG", config_path_for(&root))
        .env("WT_FZF", &fzf)
        .output()
        .expect("spawn wt");

    assert!(
        out.status.success(),
        "expected a cancelled pick to exit cleanly: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "expected no error output, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn launch_branch_with_no_worktree_errors_about_needing_a_name() {
    let tmp = unique_dir("launch-picker-branch");
    let root = tmp.join("wt-root");

    let out = run_wt(&root, &["launch", "--branch", "foo"]);
    assert!(!out.status.success(), "expected launch to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--branch") && stderr.contains("worktree name"),
        "expected a message about needing a worktree name, got: {stderr}"
    );
}

#[test]
fn launch_preview_prints_a_provisioned_trees_details() {
    let tmp = unique_dir("launch-preview");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    assert_success(
        &run_wt(&root, &["new", "myrepo", "--name", "preview target"]),
        "new",
    );
    assert_success(&run_wt(&root, &["wait", "preview target"]), "wait");

    let path_out = run_wt(&root, &["path", "preview target"]);
    assert_success(&path_out, "path");
    let tree_path = String::from_utf8_lossy(&path_out.stdout).trim().to_string();

    let out = run_wt(&root, &["__launch-preview", "preview target"]);
    assert_success(&out, "__launch-preview");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("preview target"), "missing name: {stdout}");
    assert!(stdout.contains("myrepo"), "missing repo: {stdout}");
    assert!(
        stdout.contains("josh/preview-target"),
        "missing branch: {stdout}"
    );
    assert!(stdout.contains(&tree_path), "missing path: {stdout}");
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

/// Hand-builds a `.graphite_metadata.db` in `base`'s git dir tracking
/// `master -> a -> b -> c`, with no row for `c` needing to resolve to a
/// worktree — the same shape `stack.rs`'s own fixtures use.
fn write_graphite_stack(base: &Path) {
    let db = base.join(".git").join(".graphite_metadata.db");
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
         ('master', NULL, 'TRUNK'), ('a', 'master', NULL), ('b', 'a', NULL), ('c', 'b', NULL);",
    );
}

#[test]
fn session_context_reports_stack_position_in_a_mid_stack_tree() {
    let tmp = unique_dir("ctx-stack");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    assert_success(
        &run_wt(
            &root,
            &["new", "myrepo", "--name", "tree a", "--branch", "a"],
        ),
        "new tree a",
    );
    assert_success(
        &run_wt(
            &root,
            &["new", "myrepo", "--name", "tree b", "--branch", "b"],
        ),
        "new tree b",
    );
    assert_success(&run_wt(&root, &["wait", "tree a"]), "wait tree a");
    assert_success(&run_wt(&root, &["wait", "tree b"]), "wait tree b");
    write_graphite_stack(&base);

    let path_out = run_wt(&root, &["path", "tree b"]);
    assert_success(&path_out, "path tree b");
    let tree_b_path = String::from_utf8_lossy(&path_out.stdout).trim().to_string();

    let out = run_wt(&root, &["__session-context", "--path", &tree_b_path]);
    assert_success(&out, "__session-context");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("This directory is the wt tree \"tree b\" (branch b) of repo myrepo."),
        "missing header: {stdout}"
    );
    assert!(
        stdout.contains("Below this branch in the stack: 'a', held by tree \"tree a\"."),
        "missing parent line: {stdout}"
    );
    assert!(
        stdout.contains("This tree is mid-stack: 'a' belongs to tree \"tree a\", not this one."),
        "missing mid-stack warning: {stdout}"
    );
    assert!(
        stdout.contains("Stacked on top of this branch: 'c' (no worktree right now)."),
        "missing children line: {stdout}"
    );
}

#[test]
fn session_context_stack_lines_are_absent_without_graphite() {
    let tmp = unique_dir("ctx-no-graphite");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");

    init_repo(&root, "myrepo", &base);
    assert_success(
        &run_wt(&root, &["new", "myrepo", "--name", "plain tree"]),
        "new",
    );
    assert_success(&run_wt(&root, &["wait", "plain tree"]), "wait");
    // No `.graphite_metadata.db` written — this repo never ran `gt`.

    let path_out = run_wt(&root, &["path", "plain tree"]);
    assert_success(&path_out, "path");
    let tree_path = String::from_utf8_lossy(&path_out.stdout).trim().to_string();

    let out = run_wt(&root, &["__session-context", "--path", &tree_path]);
    assert_success(&out, "__session-context");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("This directory is the wt tree \"plain tree\""),
        "base context missing: {stdout}"
    );
    assert!(
        !stdout.contains("Below this branch"),
        "must not show stack lines with no Graphite: {stdout}"
    );
    assert!(
        !stdout.contains("Stacked on top of"),
        "must not show stack lines with no Graphite: {stdout}"
    );
}

#[test]
fn a_ready_spare_at_the_same_commit_is_claimed_instantly_with_no_step_rerun() {
    let tmp = unique_dir("spare-instant");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    let marker = tmp.join("marker.txt");
    inject_step(
        &root,
        "myrepo",
        "mark",
        &[
            "sh",
            "-c",
            &format!("printf 'run\\n' >> {}", marker.display()),
        ],
    );
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");
    assert_eq!(
        marker_run_count(&marker),
        1,
        "the spare's own build should have run the step once"
    );
    let spare_path = PathBuf::from(spare["path"].as_str().unwrap());
    // A replacement build after the claim reuses the same injected step,
    // so it would add its own marker line unless spares are off first.
    set_spares(&root, "myrepo", 0);

    let out = run_wt(&root, &["new", "myrepo", "--name", "instant claim"]);
    assert_success(&out, "new (instant claim)");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_eq!(
        tree_path, spare_path,
        "an instant claim should reuse the spare's own worktree"
    );

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    assert_eq!(entries[0]["name"], "instant claim");
    assert_eq!(
        entries[0]["state"], "ready",
        "an instant claim needs no provisioning wait"
    );
    assert_eq!(
        marker_run_count(&marker),
        1,
        "an instant claim must not re-run the provisioning step"
    );

    assert_success(
        &run_wt(&root, &["rm", "instant claim", "--delete-branch"]),
        "cleanup tree",
    );
}

#[test]
fn a_spare_behind_a_moved_trunk_is_claimed_but_reruns_its_steps() {
    let tmp = unique_dir("spare-warm");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    let marker = tmp.join("marker.txt");
    inject_step(
        &root,
        "myrepo",
        "mark",
        &[
            "sh",
            "-c",
            &format!("printf 'run\\n' >> {}", marker.display()),
        ],
    );
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");
    assert_eq!(marker_run_count(&marker), 1);
    let spare_path = PathBuf::from(spare["path"].as_str().unwrap());
    // A replacement build after the claim reuses the same injected step,
    // so it would add its own marker line unless spares are off first.
    set_spares(&root, "myrepo", 0);

    push_new_commit_to_origin(&tmp.join("origin.git"), &tmp, "advance.txt");
    git(&["fetch", "-q", "origin"], &base);

    let out = run_wt(&root, &["new", "myrepo", "--name", "warm claim"]);
    assert_success(&out, "new (warm claim)");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_eq!(
        tree_path, spare_path,
        "a warm claim should still reuse the spare's own worktree"
    );

    assert_success(&run_wt(&root, &["wait", "warm claim"]), "wait");
    assert_eq!(
        marker_run_count(&marker),
        2,
        "a warm claim must rerun the provisioning step once"
    );
    assert!(
        tree_path.join("advance.txt").exists(),
        "the claimed tree should be checked out at the new trunk commit"
    );

    assert_success(
        &run_wt(&root, &["rm", "warm claim", "--delete-branch"]),
        "cleanup tree",
    );
}

#[test]
fn two_concurrent_new_calls_against_one_spare_both_succeed_and_only_one_claims_it() {
    let tmp = unique_dir("spare-concurrent");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");
    let spare_path = PathBuf::from(spare["path"].as_str().unwrap());

    let child_a = spawn_wt(&root, &["new", "myrepo", "--name", "concurrent a"]);
    let child_b = spawn_wt(&root, &["new", "myrepo", "--name", "concurrent b"]);
    let out_a = child_a.wait_with_output().expect("wait concurrent a");
    let out_b = child_b.wait_with_output().expect("wait concurrent b");
    assert_success(&out_a, "new concurrent a");
    assert_success(&out_b, "new concurrent b");

    let path_a = PathBuf::from(
        String::from_utf8_lossy(&out_a.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    let path_b = PathBuf::from(
        String::from_utf8_lossy(&out_b.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_ne!(
        path_a, path_b,
        "the two claims must land in distinct worktrees"
    );

    assert_success(&run_wt(&root, &["wait", "concurrent a"]), "wait a");
    assert_success(&run_wt(&root, &["wait", "concurrent b"]), "wait b");

    let claimed_the_spare = [&path_a, &path_b]
        .iter()
        .filter(|p| ***p == spare_path)
        .count();
    assert_eq!(
        claimed_the_spare, 1,
        "exactly one of the two calls should have claimed the spare's own path"
    );

    let ls = run_wt(&root, &["ls", "--json"]);
    assert_success(&ls, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&ls.stdout).unwrap();
    let entries = entries.as_array().unwrap();
    let branch_of = |name: &str| -> String {
        entries.iter().find(|e| e["name"] == name).unwrap()["branch"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_ne!(branch_of("concurrent a"), branch_of("concurrent b"));

    wait_for_all_spares_settled(&root, "myrepo", 60);
    assert_success(
        &run_wt(&root, &["rm", "concurrent a", "--delete-branch"]),
        "cleanup a",
    );
    assert_success(
        &run_wt(&root, &["rm", "concurrent b", "--delete-branch"]),
        "cleanup b",
    );
    assert_success(
        &run_wt(&root, &["spare", "drop", "--repo", "myrepo"]),
        "cleanup spare",
    );
}

#[test]
fn a_repo_never_has_more_than_its_configured_number_of_spares_after_a_claim_tops_up() {
    let tmp = unique_dir("spare-no-dup");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    build_claim_and_wait(&root, "myrepo", "dup check");
    let rows = wait_for_all_spares_settled(&root, "myrepo", 60);
    assert_eq!(
        rows.len(),
        1,
        "myrepo's spares: 1 must never grow past one row: {rows:?}"
    );

    assert_success(
        &run_wt(&root, &["rm", "dup check", "--delete-branch"]),
        "cleanup tree",
    );
    assert_success(
        &run_wt(&root, &["spare", "drop", "--repo", "myrepo"]),
        "cleanup spare",
    );
}

#[test]
fn a_broken_spare_pointing_at_a_deleted_directory_falls_back_to_a_cold_build() {
    let tmp = unique_dir("spare-broken");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");

    mutate_tree_json(&root, SPARE_NAME, |t| {
        t["path"] = serde_json::json!("/nonexistent-deleted-spare-dir");
    });

    let out = run_wt(&root, &["new", "myrepo", "--name", "cold fallback"]);
    assert_success(&out, "new (cold fallback)");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert!(
        tree_path.join(".git").exists(),
        "the cold path must still build a real worktree"
    );
    assert_success(&run_wt(&root, &["wait", "cold fallback"]), "wait");

    let rows = spare_rows(&root, "myrepo");
    assert_eq!(
        rows.len(),
        1,
        "the broken spare row must survive untouched: {rows:?}"
    );
    assert_eq!(rows[0]["state"], "ready");
    assert_eq!(
        rows[0]["path"], "/nonexistent-deleted-spare-dir",
        "a failed claim attempt must never rewrite the spare row it gave up on"
    );

    assert_success(
        &run_wt(&root, &["rm", "cold fallback", "--delete-branch"]),
        "cleanup tree",
    );
}

#[test]
fn gc_never_reaps_a_spare_that_is_clean_with_no_commits() {
    let tmp = unique_dir("spare-gc");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");
    let spare_path = PathBuf::from(spare["path"].as_str().unwrap());

    let out = run_wt(&root, &["gc"]);
    assert_success(&out, "gc");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("nothing to reap"),
        "gc should find nothing to reap with only a hot spare registered"
    );
    assert!(
        spare_path.exists(),
        "gc must never remove a hot spare's worktree"
    );

    let rows = spare_rows(&root, "myrepo");
    assert_eq!(
        rows.len(),
        1,
        "gc must not reap the spare's registry row either"
    );
}

#[test]
fn wait_with_no_selector_ignores_a_provisioning_spare() {
    let tmp = unique_dir("spare-wait");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");

    mutate_tree_json(&root, SPARE_NAME, |t| {
        t["state"] = serde_json::json!("provisioning");
        t["provisionPid"] = serde_json::json!(999_999);
    });

    let out = run_wt(&root, &["wait"]);
    assert!(
        !out.status.success(),
        "expected wait to fail with only a provisioning spare registered"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no tree is provisioning"),
        "wait must not pick a spare with no selector: {stderr}"
    );

    assert_success(
        &run_wt(&root, &["spare", "drop", "--repo", "myrepo"]),
        "cleanup spare",
    );
}

#[test]
fn a_replacement_spare_appears_on_its_own_after_a_claim() {
    let tmp = unique_dir("spare-replacement");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let original_id = build_claim_and_wait(&root, "myrepo", "replacement check");
    let rows = wait_for_all_spares_settled(&root, "myrepo", 60);
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one replacement spare: {rows:?}"
    );
    assert_ne!(
        rows[0]["id"].as_str().unwrap(),
        original_id,
        "the replacement must be a fresh spare, not the one that was claimed"
    );
    assert_eq!(rows[0]["state"], "ready");

    assert_success(
        &run_wt(&root, &["rm", "replacement check", "--delete-branch"]),
        "cleanup tree",
    );
    assert_success(
        &run_wt(&root, &["spare", "drop", "--repo", "myrepo"]),
        "cleanup spare",
    );
}

#[test]
fn spares_set_to_zero_creates_no_spare_and_the_cold_path_is_unchanged() {
    let tmp = unique_dir("spare-zero");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base); // spares: 0, set by init_repo itself

    assert_eq!(spare_rows(&root, "myrepo").len(), 0);

    let out = run_wt(&root, &["new", "myrepo", "--name", "cold as usual"]);
    assert_success(&out, "new");
    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_success(&run_wt(&root, &["wait", "cold as usual"]), "wait");
    assert!(tree_path.join(".git").exists());

    assert_eq!(
        spare_rows(&root, "myrepo").len(),
        0,
        "spares: 0 must never build one, even after wt new's own top-up call"
    );

    assert_success(
        &run_wt(&root, &["rm", "cold as usual", "--delete-branch"]),
        "cleanup",
    );
}

#[test]
fn spare_drop_survives_a_sync_top_up() {
    let tmp = unique_dir("spare-drop-durable");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    // Timed so the behavioral check below can wait comfortably longer than
    // a real build takes on this fixture, instead of guessing a constant.
    let build_started = std::time::Instant::now();
    build_and_wait_spare(&root, "myrepo", 60);
    let build_elapsed = build_started.elapsed();
    assert_eq!(spare_rows(&root, "myrepo").len(), 1);

    assert_success(
        &run_wt(&root, &["spare", "drop", "--repo", "myrepo"]),
        "spare drop",
    );
    assert_eq!(spare_rows(&root, "myrepo").len(), 0);

    // Deterministic half: this is the actual guard, independent of any
    // timing — `wt spare drop` must persist `spares 0`, not just clear the
    // rows in state.
    let config_text = std::fs::read_to_string(config_path_for(&root)).unwrap();
    let body = repo_block_body(&config_text, "myrepo");
    assert!(
        config_text[body].contains("spares 0"),
        "wt spare drop must set spares to 0 in config.kdl: {config_text}"
    );

    // Behavioral half: prove a sync tick doesn't rebuild one, by actually
    // waiting rather than checking once right after `sync` returns — the
    // spawned top-up child registers its row asynchronously, so a single
    // immediate check passes whether or not the fix is in.
    assert_success(&run_wt(&root, &["sync", "myrepo"]), "sync after drop");
    let window = (build_elapsed * 3).max(std::time::Duration::from_secs(5));
    let deadline = std::time::Instant::now() + window;
    loop {
        let rows = spare_rows(&root, "myrepo");
        assert_eq!(
            rows.len(),
            0,
            "a sync tick must not rebuild a spare `wt spare drop` just turned off: {rows:?}"
        );
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn a_dirty_spare_working_tree_falls_back_to_cold_and_leaves_the_spare_row_untouched() {
    let tmp = unique_dir("spare-dirty");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");
    let spare_path = PathBuf::from(spare["path"].as_str().unwrap());

    push_readme_commit(&tmp.join("origin.git"), &tmp, "origin edit\n");
    git(&["fetch", "-q", "origin"], &base);
    std::fs::write(spare_path.join("README.md"), "dirty local edit\n").unwrap();

    let out = run_wt(&root, &["new", "myrepo", "--name", "dirty fallback"]);
    assert_success(&out, "new (dirty fallback)");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("building the tree from cold instead"),
        "expected a fallback warning: {stderr}"
    );

    let tree_path = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    assert_ne!(
        tree_path, spare_path,
        "a dirty claim must build a fresh worktree, not reuse the spare's"
    );
    assert_success(&run_wt(&root, &["wait", "dirty fallback"]), "wait");

    let rows = spare_rows(&root, "myrepo");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["state"], "ready",
        "a failed claim must leave the spare row exactly as it was"
    );
    assert_eq!(
        std::fs::read_to_string(spare_path.join("README.md")).unwrap(),
        "dirty local edit\n",
        "the spare's dirty working tree must be untouched by the failed claim"
    );

    assert_success(
        &run_wt(&root, &["rm", "dirty fallback", "--delete-branch"]),
        "cleanup tree",
    );
}

#[test]
fn status_with_no_selector_hides_spares_and_all_shows_them() {
    let tmp = unique_dir("spare-status");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");

    mutate_tree_json(&root, SPARE_NAME, |t| {
        t["state"] = serde_json::json!("failed");
    });

    let hidden = run_wt(&root, &["status", "--json"]);
    assert_success(&hidden, "status --json");
    let entries: serde_json::Value = serde_json::from_slice(&hidden.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        0,
        "status with no selector must hide a non-ready spare"
    );

    let shown = run_wt(&root, &["status", "--all", "--json"]);
    assert_success(&shown, "status --all --json");
    let entries: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    let entries = entries.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], SPARE_NAME);
    assert_eq!(entries[0]["state"], "failed");
}

#[test]
fn ls_hides_spares_all_shows_them_and_json_carries_the_spare_field() {
    let tmp = unique_dir("spare-ls");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");

    let hidden = run_wt(&root, &["ls", "--json"]);
    assert_success(&hidden, "ls --json");
    let entries: serde_json::Value = serde_json::from_slice(&hidden.stdout).unwrap();
    assert_eq!(
        entries.as_array().unwrap().len(),
        0,
        "ls with no --all must hide the spare"
    );

    let hidden_text = run_wt(&root, &["ls"]);
    assert_success(&hidden_text, "ls");
    assert!(!String::from_utf8_lossy(&hidden_text.stdout).contains(SPARE_NAME));

    let shown = run_wt(&root, &["ls", "--all", "--json"]);
    assert_success(&shown, "ls --all --json");
    let entries: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    let entries = entries.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], SPARE_NAME);
    assert_eq!(entries[0]["spare"], true);

    let shown_text = run_wt(&root, &["ls", "--all"]);
    assert_success(&shown_text, "ls --all");
    let stdout = String::from_utf8_lossy(&shown_text.stdout);
    assert!(stdout.contains(SPARE_NAME));
    assert!(
        stdout.contains("spare"),
        "the STATE column should render a ready spare as 'spare': {stdout}"
    );
}

#[test]
fn doctor_reports_nothing_about_a_detached_spare() {
    let tmp = unique_dir("spare-doctor");
    let base = fixture_repo(&tmp);
    let root = tmp.join("wt-root");
    init_repo(&root, "myrepo", &base);
    enable_spares(&root, "myrepo", 1);

    let spare = build_and_wait_spare(&root, "myrepo", 60);
    assert_eq!(spare["state"], "ready");

    let out = run_wt(&root, &["doctor"]);
    assert_success(&out, "doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("branch mismatch"),
        "a detached spare must never be flagged as a branch mismatch: {stdout}"
    );
    assert!(
        !stdout.contains("unregistered worktree"),
        "the spare's own worktree must be recognized, not flagged as unregistered: {stdout}"
    );
}

/// Deletes every regular file under `dir`, leaving its directory tree in
/// place — reproducing what the OS temp-file pruner does to a long-idle
/// cache.
fn delete_regular_files(dir: &Path) {
    for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
        let path = entry.path();
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            delete_regular_files(&path);
        } else {
            std::fs::remove_file(&path).expect("remove file");
        }
    }
}

#[test]
fn fixture_template_rebuilds_after_files_are_pruned_from_it() {
    // A private path, not the shared `fixture_template()` one: the suite
    // runs in parallel, so purging the shared template would break every
    // other test mid-run.
    let tmp = unique_dir("template-purge");
    let template_path = tmp.join("template");

    ensure_fixture_template(&template_path);
    assert!(template_is_valid(&template_path));

    delete_regular_files(&template_path.join("work"));
    assert!(
        !template_is_valid(&template_path),
        "a hollow work dir must fail validation even with an intact origin.git"
    );
    ensure_fixture_template(&template_path);
    assert!(template_is_valid(&template_path));

    delete_regular_files(&template_path.join("origin.git"));
    assert!(
        !template_is_valid(&template_path),
        "a hollow origin.git must fail validation even with an intact work dir"
    );
    ensure_fixture_template(&template_path);
    assert!(template_is_valid(&template_path));

    delete_regular_files(&template_path);
    assert!(!template_is_valid(&template_path));

    let rebuilt = ensure_fixture_template(&template_path);
    assert!(template_is_valid(&rebuilt));

    let work = tmp.join("work");
    clone_tree(&rebuilt.join("work"), &work);
    let head = git_rev_parse(&work, "HEAD");
    assert_eq!(
        head.len(),
        40,
        "rebuilt template's work dir has no resolvable HEAD: {head:?}"
    );
}
