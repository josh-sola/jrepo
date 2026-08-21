use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::config;
use crate::store::{self, TreeState};

/// Runs as `wt __provision <tree-id>`, re-exec'd and detached by `wt tree new`
/// so the parent can return the tree path immediately. All state lives in
/// `state.json`; `wt tree status`/`wt tree wait` read it back, no IPC needed.
pub fn run(
    root: &Path,
    config_path: &Path,
    tree_id: Uuid,
    profiles: Option<Vec<String>>,
) -> Result<()> {
    let store = store::load(root)?;
    let tree = store
        .trees
        .iter()
        .find(|t| t.id == tree_id)
        .with_context(|| format!("tree {tree_id} not found in registry"))?;
    let config = config::load(config_path)?;
    run_steps(root, tree_id, &tree.path, &tree.repo, &config, &profiles)
}

/// Opens the tree's `.wt-provision.log` here rather than inheriting stdio
/// the caller redirected at spawn time: a hot spare's tree path isn't known
/// until long after its detached build is already running.
pub(crate) fn run_steps(
    root: &Path,
    tree_id: Uuid,
    tree_path: &Path,
    repo_name: &str,
    config: &config::Config,
    profiles: &Option<Vec<String>>,
) -> Result<()> {
    let repo_config = config::repo(config, repo_name)?;

    let log_path = tree_path.join(crate::repo::PROVISION_LOG_NAME);
    let mut log =
        File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?;

    let steps: Vec<_> = repo_config
        .steps
        .iter()
        .filter(|s| match profiles {
            None => true,
            Some(profiles) => profiles.iter().any(|p| p == &s.profile),
        })
        .cloned()
        .collect();
    let total = steps.len() as u32;
    // Lowest priority first: a machine-computed default, then the config's
    // global env, then the repo's own — each later layer can override
    // anything an earlier one set.
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("PATH".to_string(), crate::repo::steps_path());
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        root.join(repo_name)
            .join("cache")
            .join("cargo-target")
            .to_string_lossy()
            .to_string(),
    );
    env.extend(config.env.clone());
    env.extend(repo_config.env.clone());

    for (i, step) in steps.iter().enumerate() {
        let index = i as u32 + 1;
        writeln!(log, "[{index}/{total}] {}", step.label).ok();
        store::with_store_lock(root, |s| {
            if let Some(t) = s.trees.iter_mut().find(|t| t.id == tree_id) {
                t.step_label = Some(step.label.clone());
                t.step_index = Some(index);
                t.step_total = Some(total);
            }
            Ok(())
        })?;

        let stdout = log
            .try_clone()
            .with_context(|| format!("cloning handle for {}", log_path.display()))?;
        let stderr = log
            .try_clone()
            .with_context(|| format!("cloning handle for {}", log_path.display()))?;
        // A command that cannot even start — the usual cause is a tool the
        // caller's PATH doesn't reach — has to land in `Failed` like any
        // other step failure. Letting the error escape instead leaves the
        // tree `provisioning` behind a pid that is already gone, which
        // reads as merely wedged, so nothing reports it and the work is
        // retried forever.
        let outcome = Command::new(&step.cmd[0])
            .args(&step.cmd[1..])
            .current_dir(tree_path.join(&step.cwd))
            .envs(&env)
            .stdout(stdout)
            .stderr(stderr)
            .status();

        let failure = match outcome {
            Err(e) => Some(format!("step '{}' could not start: {e}", step.label)),
            Ok(status) if !status.success() => Some(format!("step '{}' failed", step.label)),
            Ok(_) => None,
        };

        if let Some(reason) = failure {
            writeln!(log, "{reason}").ok();
            store::with_store_lock(root, |s| {
                if let Some(t) = s.trees.iter_mut().find(|t| t.id == tree_id) {
                    t.state = TreeState::Failed;
                    t.provision_pid = None;
                }
                Ok(())
            })?;
            bail!("{reason}");
        }
    }

    store::with_store_lock(root, |s| {
        if let Some(t) = s.trees.iter_mut().find(|t| t.id == tree_id) {
            t.state = TreeState::Ready;
            t.step_label = None;
            t.step_index = None;
            t.step_total = None;
            t.provision_pid = None;
        }
        Ok(())
    })?;
    writeln!(log, "provisioning complete").ok();
    Ok(())
}
