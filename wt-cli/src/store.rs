use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use fs4::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::git;

pub const STORE_VERSION: u32 = 3;

/// The oldest `state.json` version `load` still reads. Every field added
/// since then carries a serde default, so an old file parses as-is; `load`
/// bumps the version in memory only, and the next locked mutation persists
/// it.
const MIN_LOADABLE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    #[serde(default)]
    pub repos: BTreeMap<String, Repo>,
    #[serde(default)]
    pub trees: Vec<Tree>,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            version: STORE_VERSION,
            repos: BTreeMap::new(),
            trees: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub base: PathBuf,
    #[serde(rename = "lastFetch", default, skip_serializing_if = "Option::is_none")]
    pub last_fetch: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    pub id: Uuid,
    pub repo: String,
    pub name: String,
    pub branch: String,
    pub path: PathBuf,
    pub created: DateTime<Utc>,
    pub state: TreeState,
    #[serde(rename = "stepLabel", default, skip_serializing_if = "Option::is_none")]
    pub step_label: Option<String>,
    #[serde(rename = "stepIndex", default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<u32>,
    #[serde(rename = "stepTotal", default, skip_serializing_if = "Option::is_none")]
    pub step_total: Option<u32>,
    #[serde(rename = "logPath", default, skip_serializing_if = "Option::is_none")]
    pub log_path: Option<PathBuf>,
    /// The `wt __provision` child's pid, so a teardown mid-provisioning can
    /// stop it before deleting the directory out from under it. `None`
    /// once provisioning reaches `ready` or `failed`.
    #[serde(
        rename = "provisionPid",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub provision_pid: Option<u32>,
    /// This tree's parent branch in its Graphite stack, kept apart from
    /// Graphite's own record of parentage so a teardown check still knows a
    /// tree's stack position when Graphite's database is missing or stale.
    /// Maintained for the tree's life, not just written at creation.
    #[serde(
        rename = "parentBranch",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_branch: Option<String>,
    /// `parent_branch`'s head commit at the moment this tree was created,
    /// used to detect a needed restack without querying Graphite's db.
    #[serde(
        rename = "parentRevision",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_revision: Option<String>,
    /// A restack of this tree's branch is known-needed but hasn't run yet.
    /// The session hook reads this instead of deriving `needs_restack`,
    /// which needs a live-head and merge-base check too expensive to pay on
    /// every prompt; the restack walk and `wt sync` are what set and clear
    /// it.
    #[serde(rename = "pendingRestack", default, skip_serializing_if = "is_false")]
    pub pending_restack: bool,
    /// This branch's pull request number, recorded once `wt submit`
    /// succeeds. A PR's state is read fresh from the `.graphite_pr_info`
    /// sidecar instead — recording a snapshot of it here would just go
    /// stale.
    #[serde(rename = "prNumber", default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    /// An unclaimed hot spare: provisioned ahead of time, sitting on a
    /// detached HEAD, hidden from listings and never reaped. Claiming one
    /// clears this, and the row becomes an ordinary tree.
    ///
    /// A spare in `Ready` has finished every provisioning step *at its
    /// current HEAD*. That is what lets a claim off the same commit skip the
    /// steps entirely, so nothing may move a spare's HEAD without re-running
    /// them in the same operation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub spare: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Display name for an unclaimed spare, shown by `wt tree ls --all`. It carries
/// no meaning for lookups: `resolve_index` skips spares outright, so this
/// never has to be unique or collision-proof.
pub const SPARE_NAME: &str = "@spare";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeState {
    Provisioning,
    Ready,
    Failed,
}

/// `$WT_ROOT` lets tests and manual smoke runs point at a throwaway
/// directory instead of the real `~/repos/wt`.
pub fn root_dir() -> PathBuf {
    if let Ok(p) = env::var("WT_ROOT") {
        return PathBuf::from(p);
    }
    let home = env::var("HOME").expect("HOME must be set");
    PathBuf::from(home).join("repos").join("wt")
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join("state.json")
}

/// Named apart from `state.json` on purpose: renaming it too would let an
/// old binary and a new one hold different locks during the upgrade.
fn lock_path(root: &Path) -> PathBuf {
    root.join(".data.lock")
}

/// A missing `state.json` is an empty store, not an error — a fresh
/// `$WT_ROOT` is the normal starting state.
pub fn load(root: &Path) -> Result<Store> {
    let path = state_path(root);
    match fs::read(&path) {
        Ok(bytes) => {
            let mut store: Store = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            if store.version < MIN_LOADABLE_VERSION || store.version > STORE_VERSION {
                bail!(
                    "{} has version {}, but wt reads versions {MIN_LOADABLE_VERSION}-{STORE_VERSION}",
                    path.display(),
                    store.version
                );
            }
            // Bumped in memory only: `load` runs without the store lock, so
            // saving here could race a concurrent locked writer and clobber
            // its update. The next `with_store_lock` mutation persists v3.
            store.version = STORE_VERSION;
            Ok(store)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn save(root: &Path, store: &Store) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let tmp = root.join("state.json.tmp");
    let bytes = serde_json::to_vec_pretty(store)?;
    let mut file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    // Without this, `rename` can land before the bytes do, so a crash
    // between the two can leave `state.json` empty or truncated.
    file.sync_all()
        .with_context(|| format!("syncing {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, state_path(root))
        .with_context(|| format!("renaming {} into place", tmp.display()))?;
    Ok(())
}

/// Concurrent `wt tree new` calls from separate processes serialize on the
/// lockfile rather than racing on `state.json` directly.
pub fn with_store_lock<F, R>(root: &Path, f: F) -> Result<R>
where
    F: FnOnce(&mut Store) -> Result<R>,
{
    fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(root))
        .with_context(|| format!("opening {}", lock_path(root).display()))?;
    // Qualified so this exercises fs4's flock wrapper rather than the
    // inherent std::fs::File::lock stabilized in later toolchains.
    FileExt::lock(&lock_file).context("acquiring state.json lock")?;
    let mut store = load(root)?;
    let result = f(&mut store)?;
    save(root, &store)?;
    FileExt::unlock(&lock_file).context("releasing state.json lock")?;
    Ok(result)
}

/// Ambiguity within a tier is an error, not a fallthrough — a broader tier
/// below never gets a chance to silently pick a winner.
pub fn resolve<'a>(trees: &'a [Tree], selector: &str) -> Result<&'a Tree> {
    resolve_optional(trees, selector)?
        .with_context(|| format!("no tree matches selector '{selector}'"))
}

pub fn resolve_optional<'a>(trees: &'a [Tree], selector: &str) -> Result<Option<&'a Tree>> {
    Ok(resolve_index_optional(trees, selector)?.map(|idx| &trees[idx]))
}

fn resolve_index_optional(trees: &[Tree], selector: &str) -> Result<Option<usize>> {
    let needle = selector.to_lowercase();
    let tiers: [fn(&Tree, &str, &str) -> bool; 6] = [
        |t, s, _| t.id.to_string() == s,
        |t, _, needle| t.id.to_string().starts_with(needle),
        |t, s, _| t.name == s,
        |t, _, needle| t.name.to_lowercase().contains(needle),
        |t, s, _| t.branch == s,
        // `gt create` moves a tree onto a new branch without updating the
        // registry, so the branch it started on and the branch it's
        // actually on can differ; this tier is what lets a selector still
        // find the tree once that's happened.
        |t, s, _| live_branch(t).as_deref() == Some(s),
    ];
    for tier in tiers {
        let matches: Vec<usize> = trees
            .iter()
            .enumerate()
            // Spares are unclaimed and about to be rewritten by whoever
            // claims them, so no user selector may land on one.
            .filter(|(_, t)| !t.spare)
            .filter(|(_, t)| tier(t, selector, &needle))
            .map(|(i, _)| i)
            .collect();
        match matches.len() {
            0 => continue,
            1 => return Ok(Some(matches[0])),
            _ => {
                let candidates = matches
                    .iter()
                    .map(|&i| format!("{} ({})", trees[i].name, trees[i].id))
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("selector '{selector}' is ambiguous: {candidates}");
            }
        }
    }
    Ok(None)
}

/// `None` on any git failure (path missing, still provisioning, not a
/// worktree yet) rather than falling back to the recorded branch — a
/// silent fallback here would make this tier indistinguishable from the
/// recorded-branch tier above it. Callers that want a branch unconditionally
/// fall back to `Tree.branch` themselves: `resolve_index` needs `None` to
/// stay distinct from the recorded-branch tier, so it can't default here.
pub fn live_branch(t: &Tree) -> Option<String> {
    git::current_branch(&t.path).ok()
}

/// The longest matching tree path wins, so a tree nested under another
/// repo's base resolves to its own repo rather than the enclosing one.
pub fn repo_for_cwd<'a>(store: &'a Store, cwd: &Path) -> Option<&'a str> {
    if let Some(tree) = store
        .trees
        .iter()
        .filter(|t| cwd.starts_with(&t.path))
        .max_by_key(|t| t.path.components().count())
    {
        return Some(&tree.repo);
    }
    store
        .repos
        .iter()
        .find(|(_, r)| cwd.starts_with(&r.base))
        .map(|(name, _)| name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wt-store-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_tree(name: &str, branch: &str) -> Tree {
        Tree {
            id: Uuid::now_v7(),
            repo: "monorepo".into(),
            name: name.into(),
            branch: branch.into(),
            path: PathBuf::from("/tmp/x"),
            created: Utc::now(),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
            parent_branch: None,
            parent_revision: None,
            pending_restack: false,
            pr_number: None,
            spare: false,
        }
    }

    #[test]
    fn round_trip_and_atomic_write() {
        let root = temp_root();
        let mut store = Store::default();
        store
            .trees
            .push(sample_tree("scratch test", "josh/scratch-test"));
        save(&root, &store).unwrap();

        assert!(!root.join("state.json.tmp").exists());
        let loaded = load(&root).unwrap();
        assert_eq!(loaded.trees.len(), 1);
        assert_eq!(loaded.trees[0].name, "scratch test");
    }

    #[test]
    fn pending_restack_round_trips_and_stays_out_of_json_when_false() {
        let root = temp_root();
        let mut store = Store::default();
        let mut tree = sample_tree("needs one", "josh/needs-one");
        tree.pending_restack = true;
        store.trees.push(tree);
        save(&root, &store).unwrap();

        let on_disk = fs::read_to_string(state_path(&root)).unwrap();
        assert!(on_disk.contains("\"pendingRestack\": true"), "{on_disk}");

        let loaded = load(&root).unwrap();
        assert!(loaded.trees[0].pending_restack);

        // A tree with no pending restack writes nothing for it at all —
        // the field is opt-in noise, not a default every row carries.
        let mut store = Store::default();
        store.trees.push(sample_tree("clean", "josh/clean"));
        save(&root, &store).unwrap();
        let on_disk = fs::read_to_string(state_path(&root)).unwrap();
        assert!(!on_disk.contains("pendingRestack"), "{on_disk}");
        assert!(!load(&root).unwrap().trees[0].pending_restack);
    }

    #[test]
    fn missing_state_json_is_empty_store() {
        let root = temp_root();
        let store = load(&root).unwrap();
        assert_eq!(store.trees.len(), 0);
        assert_eq!(store.version, STORE_VERSION);
    }

    #[test]
    fn load_rejects_an_unsupported_version() {
        let root = temp_root();
        fs::write(state_path(&root), r#"{"version": 99}"#).unwrap();
        let err = load(&root).unwrap_err();
        assert!(err.to_string().contains("version 99"), "message was: {err}");
    }

    #[test]
    fn load_upgrades_a_v2_store_in_memory_without_writing_to_disk() {
        let root = temp_root();
        let tree = sample_tree("scratch test", "josh/scratch-test");
        let v2 = serde_json::json!({
            "version": 2,
            "repos": {},
            "trees": [tree],
        });
        fs::write(state_path(&root), serde_json::to_vec(&v2).unwrap()).unwrap();

        let loaded = load(&root).unwrap();
        assert_eq!(loaded.version, STORE_VERSION);
        assert_eq!(loaded.trees.len(), 1);
        assert_eq!(loaded.trees[0].parent_revision, None);

        // `load` runs without the store lock, so it must never write —
        // doing so could race and clobber a concurrent locked writer.
        let on_disk: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(state_path(&root)).unwrap()).unwrap();
        assert_eq!(on_disk["version"], 2);

        // The next real mutation, taken under the lock, persists the
        // upgrade to disk.
        with_store_lock(&root, |s| {
            s.trees.push(sample_tree("second", "josh/second"));
            Ok(())
        })
        .unwrap();
        let on_disk_after_mutation: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(state_path(&root)).unwrap()).unwrap();
        assert_eq!(on_disk_after_mutation["version"], STORE_VERSION);
    }

    #[test]
    fn concurrent_appends_do_not_lose_entries() {
        let root = temp_root();
        let n = 16;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let root = root.clone();
                thread::spawn(move || {
                    with_store_lock(&root, |store| {
                        store
                            .trees
                            .push(sample_tree(&format!("tree-{i}"), &format!("b-{i}")));
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let store = load(&root).unwrap();
        assert_eq!(store.trees.len(), n);
        let mut names: Vec<_> = store.trees.iter().map(|t| t.name.clone()).collect();
        names.sort();
        let mut expected: Vec<_> = (0..n).map(|i| format!("tree-{i}")).collect();
        expected.sort();
        assert_eq!(names, expected);
    }

    #[test]
    fn resolve_exact_uuid_then_prefix_then_name_then_substring_then_branch() {
        let mut a = sample_tree("alpha", "josh/alpha");
        let mut b = sample_tree("beta", "josh/beta");
        // uuidv7 shares a millisecond timestamp prefix across trees created
        // together, so the prefix tier needs ids that diverge immediately.
        a.id = Uuid::parse_str("aaaaaaaa-0000-7000-8000-000000000001").unwrap();
        b.id = Uuid::parse_str("bbbbbbbb-0000-7000-8000-000000000001").unwrap();
        let trees = vec![a.clone(), b.clone()];

        assert_eq!(resolve(&trees, &a.id.to_string()).unwrap().name, "alpha");
        let prefix = &a.id.to_string()[..8];
        assert_eq!(resolve(&trees, prefix).unwrap().name, "alpha");
        assert_eq!(resolve(&trees, "beta").unwrap().name, "beta");
        assert_eq!(resolve(&trees, "ALP").unwrap().name, "alpha");
        assert_eq!(resolve(&trees, "josh/beta").unwrap().name, "beta");
    }

    #[test]
    fn resolve_ambiguous_substring_lists_candidates() {
        let trees = vec![sample_tree("foo bar", "b1"), sample_tree("foo baz", "b2")];
        let err = resolve(&trees, "foo").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "message was: {msg}");
        assert!(msg.contains("foo bar"));
        assert!(msg.contains("foo baz"));
    }

    #[test]
    fn resolve_not_found() {
        let trees = vec![sample_tree("foo", "b1")];
        let err = resolve(&trees, "nope").unwrap_err();
        assert!(err.to_string().contains("no tree matches"));
    }

    #[test]
    fn resolve_cannot_reach_a_spare_by_name_substring_or_uuid_prefix() {
        let mut spare = sample_tree(SPARE_NAME, "");
        spare.spare = true;
        let id_string = spare.id.to_string();
        let trees = vec![spare];

        assert!(resolve(&trees, SPARE_NAME).is_err());
        assert!(resolve(&trees, "spare").is_err());
        assert!(resolve(&trees, &id_string).is_err());
        assert!(resolve(&trees, &id_string[..8]).is_err());
    }

    fn fixture_repo_on_branch(branch: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wt-store-git-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "."]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
        run(&["checkout", "-qb", branch]);
        dir
    }

    #[test]
    fn resolve_falls_back_to_the_tree_s_live_branch_once_it_has_moved() {
        let path = fixture_repo_on_branch("josh/moved-on");
        let mut t = sample_tree("drifted", "josh/started-here");
        t.path = path.clone();
        let trees = vec![t];

        assert_eq!(resolve(&trees, "josh/moved-on").unwrap().name, "drifted");
        // The recorded-branch tier still wins for the branch it started on.
        assert_eq!(
            resolve(&trees, "josh/started-here").unwrap().name,
            "drifted"
        );

        fs::remove_dir_all(&path).ok();
    }

    fn sample_repo(base: &str) -> Repo {
        Repo {
            base: PathBuf::from(base),
            last_fetch: None,
        }
    }

    fn cwd_store() -> Store {
        let mut store = Store::default();
        store
            .repos
            .insert("monorepo".into(), sample_repo("/r/monorepo/base"));
        store
            .repos
            .insert("toy-apps".into(), sample_repo("/r/toy-apps/base"));

        let mut outer = sample_tree("outer", "josh/outer");
        outer.path = PathBuf::from("/r/monorepo/trees/outer");
        let mut nested = sample_tree("nested", "josh/nested");
        nested.repo = "toy-apps".into();
        nested.path = PathBuf::from("/r/monorepo/trees/outer/nested");
        store.trees = vec![outer, nested];
        store
    }

    #[test]
    fn repo_for_cwd_prefers_the_longest_matching_tree_over_a_shorter_one() {
        let store = cwd_store();
        assert_eq!(
            repo_for_cwd(&store, Path::new("/r/monorepo/trees/outer/nested/src")),
            Some("toy-apps")
        );
        assert_eq!(
            repo_for_cwd(&store, Path::new("/r/monorepo/trees/outer/src")),
            Some("monorepo")
        );
    }

    #[test]
    fn repo_for_cwd_falls_back_to_a_base_then_gives_up() {
        let store = cwd_store();
        assert_eq!(
            repo_for_cwd(&store, Path::new("/r/monorepo/base/packages/api")),
            Some("monorepo")
        );
        assert_eq!(repo_for_cwd(&store, Path::new("/somewhere/else")), None);
    }
}
