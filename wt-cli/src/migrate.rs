//! One-shot upgrade from the pre-split `data.json` — config and state in one
//! file — to `state.json` plus `config.kdl`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::config;
use crate::store;

/// `data.json`'s pre-split shape, frozen here as a private mirror. Nothing
/// else in the codebase needs this shape.
#[derive(Debug, Deserialize)]
struct OldStore {
    #[serde(default)]
    repos: BTreeMap<String, OldRepo>,
    #[serde(default)]
    trees: Vec<store::Tree>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct OldRepo {
    base: PathBuf,
    trunk: String,
    #[serde(rename = "branchPrefix", default)]
    branch_prefix: String,
    #[serde(rename = "lastFetch", default)]
    last_fetch: Option<DateTime<Utc>>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    steps: Vec<OldStep>,
    #[serde(default = "config::default_spares")]
    spares: u8,
}

#[derive(Debug, Deserialize)]
struct OldStep {
    label: String,
    profile: String,
    cwd: String,
    cmd: Vec<String>,
}

/// Migrates a pre-split `data.json` into `state.json` plus `config.kdl`. A
/// no-op unless `data.json` exists and `state.json` doesn't — checked with
/// nothing heavier than two `exists()` calls, since this runs ahead of every
/// subcommand.
pub fn run_if_needed(root: &Path, config_path: &Path) -> Result<()> {
    let old_path = root.join("data.json");
    let new_path = store::state_path(root);
    if !old_path.exists() || new_path.exists() {
        return Ok(());
    }

    // Config blocks are appended, and the new state committed to disk,
    // entirely inside the closure below — before `data.json` is touched, so
    // a crash or a failing `append_repo` mid-way leaves the original in
    // place and this retriable on the next invocation.
    store::with_store_lock(root, |state| migrate_locked(config_path, &old_path, state))?;

    let migrated_path = old_path.with_extension("json.migrated");
    fs::rename(&old_path, &migrated_path)
        .with_context(|| format!("renaming {} into place", migrated_path.display()))?;
    println!(
        "migrated to {} and {}",
        new_path.display(),
        config_path.display()
    );
    Ok(())
}

fn migrate_locked(config_path: &Path, old_path: &Path, state: &mut store::Store) -> Result<()> {
    let bytes = fs::read(old_path).with_context(|| format!("reading {}", old_path.display()))?;
    let old: OldStore = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {}", old_path.display()))?;

    for (name, repo) in &old.repos {
        // PATH and CARGO_TARGET_DIR are computed at provision time, so a
        // stale absolute path must not survive into the portable config.
        let mut env = repo.env.clone();
        env.remove("CARGO_TARGET_DIR");
        let repo_config = config::RepoConfig {
            trunk: repo.trunk.clone(),
            branch_prefix: repo.branch_prefix.clone(),
            spares: repo.spares,
            env,
            steps: repo
                .steps
                .iter()
                .map(|s| config::Step {
                    label: s.label.clone(),
                    profile: s.profile.clone(),
                    cwd: s.cwd.clone(),
                    cmd: s.cmd.clone(),
                })
                .collect(),
        };
        config::append_repo(config_path, name, &repo_config)?;
    }

    let mut global_env = old.env.clone();
    global_env.remove("PATH");
    if !global_env.is_empty() {
        let keys = global_env.keys().cloned().collect::<Vec<_>>().join(", ");
        println!(
            "{} had a global env block beyond PATH ({keys}); add it to {} by hand — migration \
             only writes per-repo config blocks",
            old_path.display(),
            config_path.display()
        );
    }

    for (name, repo) in old.repos {
        state.repos.insert(
            name,
            store::Repo {
                base: repo.base,
                last_fetch: repo.last_fetch,
            },
        );
    }
    state.trees = old.trees;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wt-migrate-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_old_data_json(root: &Path) {
        fs::write(
            root.join("data.json"),
            r#"{
                "version": 1,
                "repos": {
                    "monorepo": {
                        "base": "/repos/monorepo/base",
                        "trunk": "master",
                        "branchPrefix": "josh/",
                        "lastFetch": "2026-08-05T18:00:00Z",
                        "shared": ["plans"],
                        "copy": ["**/.env*"],
                        "env": {"CARGO_TARGET_DIR": "/repos/monorepo/cache/cargo-target"},
                        "steps": [
                            {"label": "pnpm install", "profile": "node", "cwd": ".", "cmd": ["pnpm", "install"]}
                        ],
                        "spares": 2
                    }
                },
                "trees": [],
                "env": {"PATH": "/usr/bin:/bin"}
            }"#,
        )
        .unwrap();
    }

    #[test]
    fn migrates_a_v1_data_json_and_drops_computed_env() {
        let root = temp_root();
        let config_path = root.join("config.kdl");
        write_old_data_json(&root);

        run_if_needed(&root, &config_path).unwrap();

        let state = store::load(&root).unwrap();
        assert_eq!(state.version, store::STORE_VERSION);
        let repo = &state.repos["monorepo"];
        assert_eq!(repo.base, PathBuf::from("/repos/monorepo/base"));
        assert!(repo.last_fetch.is_some());

        let config = config::load(&config_path).unwrap();
        let repo_config = &config.repos["monorepo"];
        assert_eq!(repo_config.trunk, "master");
        assert_eq!(repo_config.branch_prefix, "josh/");
        assert_eq!(repo_config.spares, 2);
        assert!(!repo_config.env.contains_key("CARGO_TARGET_DIR"));
        assert_eq!(repo_config.steps.len(), 1);
        assert_eq!(repo_config.steps[0].label, "pnpm install");

        assert!(!config.env.contains_key("PATH"));
        assert!(root.join("data.json.migrated").exists());
        assert!(!root.join("data.json").exists());
    }

    #[test]
    fn second_run_is_a_noop() {
        let root = temp_root();
        let config_path = root.join("config.kdl");
        write_old_data_json(&root);

        run_if_needed(&root, &config_path).unwrap();
        let config_text_after_first = fs::read_to_string(&config_path).unwrap();

        run_if_needed(&root, &config_path).unwrap();
        let config_text_after_second = fs::read_to_string(&config_path).unwrap();

        assert_eq!(config_text_after_first, config_text_after_second);
    }

    #[test]
    fn does_not_fire_when_state_json_already_exists() {
        let root = temp_root();
        let config_path = root.join("config.kdl");
        write_old_data_json(&root);
        fs::write(store::state_path(&root), r#"{"version": 2}"#).unwrap();

        run_if_needed(&root, &config_path).unwrap();

        assert!(!config_path.exists());
        assert!(root.join("data.json").exists());
    }
}
