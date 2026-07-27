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
            version: 1,
            repos: BTreeMap::new(),
            trees: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub base: PathBuf,
    pub trunk: String,
    #[serde(rename = "branchPrefix", default)]
    pub branch_prefix: String,
    #[serde(rename = "lastFetch", default, skip_serializing_if = "Option::is_none")]
    pub last_fetch: Option<DateTime<Utc>>,
    #[serde(default)]
    pub shared: Vec<String>,
    #[serde(default)]
    pub copy: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub label: String,
    pub profile: String,
    pub cwd: String,
    pub cmd: Vec<String>,
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
}

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

pub fn data_path(root: &Path) -> PathBuf {
    root.join("data.json")
}

fn lock_path(root: &Path) -> PathBuf {
    root.join(".data.lock")
}

/// A missing `data.json` is an empty store, not an error — a fresh
/// `$WT_ROOT` is the normal starting state.
pub fn load(root: &Path) -> Result<Store> {
    let path = data_path(root);
    match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn save(root: &Path, store: &Store) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let tmp = root.join("data.json.tmp");
    let bytes = serde_json::to_vec_pretty(store)?;
    let mut file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    // Without this, `rename` can land before the bytes do, so a crash
    // between the two can leave `data.json` empty or truncated.
    file.sync_all()
        .with_context(|| format!("syncing {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, data_path(root))
        .with_context(|| format!("renaming {} into place", tmp.display()))?;
    Ok(())
}

/// Concurrent `wt new` calls from separate processes serialize on the
/// lockfile rather than racing on `data.json` directly.
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
    FileExt::lock(&lock_file).context("acquiring data.json lock")?;
    let mut store = load(root)?;
    let result = f(&mut store)?;
    save(root, &store)?;
    FileExt::unlock(&lock_file).context("releasing data.json lock")?;
    Ok(result)
}

/// Ambiguity within a tier is an error, not a fallthrough — a broader tier
/// below never gets a chance to silently pick a winner.
pub fn resolve<'a>(trees: &'a [Tree], selector: &str) -> Result<&'a Tree> {
    let idx = resolve_index(trees, selector)?;
    Ok(&trees[idx])
}

pub fn resolve_index(trees: &[Tree], selector: &str) -> Result<usize> {
    let needle = selector.to_lowercase();
    let tiers: [fn(&Tree, &str, &str) -> bool; 5] = [
        |t, s, _| t.id.to_string() == s,
        |t, _, needle| t.id.to_string().starts_with(needle),
        |t, s, _| t.name == s,
        |t, _, needle| t.name.to_lowercase().contains(needle),
        |t, s, _| t.branch == s,
    ];
    for tier in tiers {
        let matches: Vec<usize> = trees
            .iter()
            .enumerate()
            .filter(|(_, t)| tier(t, selector, &needle))
            .map(|(i, _)| i)
            .collect();
        match matches.len() {
            0 => continue,
            1 => return Ok(matches[0]),
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
    bail!("no tree matches selector '{selector}'");
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

        assert!(!root.join("data.json.tmp").exists());
        let loaded = load(&root).unwrap();
        assert_eq!(loaded.trees.len(), 1);
        assert_eq!(loaded.trees[0].name, "scratch test");
    }

    #[test]
    fn missing_data_json_is_empty_store() {
        let root = temp_root();
        let store = load(&root).unwrap();
        assert_eq!(store.trees.len(), 0);
        assert_eq!(store.version, 1);
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
}
