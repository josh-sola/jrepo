use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::store::{self, TreeState};

/// Runs as `wt __provision <tree-id>`, re-exec'd and detached by `wt new`
/// so the parent can return the tree path immediately. All state lives in
/// `data.json`; `wt status`/`wt wait` read it back, no IPC needed.
pub fn run(root: &Path, tree_id: Uuid, profiles: Option<Vec<String>>) -> Result<()> {
    let store = store::load(root)?;
    let tree = store
        .trees
        .iter()
        .find(|t| t.id == tree_id)
        .with_context(|| format!("tree {tree_id} not found in registry"))?;
    let repo = store
        .repos
        .get(&tree.repo)
        .with_context(|| format!("tree {tree_id} references unknown repo '{}'", tree.repo))?;

    let steps: Vec<_> = repo
        .steps
        .iter()
        .filter(|s| match &profiles {
            None => true,
            Some(profiles) => profiles.iter().any(|p| p == &s.profile),
        })
        .cloned()
        .collect();
    let total = steps.len() as u32;
    let tree_path = tree.path.clone();
    let env = repo.env.clone();

    for (i, step) in steps.iter().enumerate() {
        let index = i as u32 + 1;
        println!("[{index}/{total}] {}", step.label);
        store::with_store_lock(root, |s| {
            if let Some(t) = s.trees.iter_mut().find(|t| t.id == tree_id) {
                t.step_label = Some(step.label.clone());
                t.step_index = Some(index);
                t.step_total = Some(total);
            }
            Ok(())
        })?;

        let status = Command::new(&step.cmd[0])
            .args(&step.cmd[1..])
            .current_dir(tree_path.join(&step.cwd))
            .envs(&env)
            .status()
            .with_context(|| format!("running step '{}'", step.label))?;

        if !status.success() {
            eprintln!("step '{}' failed", step.label);
            store::with_store_lock(root, |s| {
                if let Some(t) = s.trees.iter_mut().find(|t| t.id == tree_id) {
                    t.state = TreeState::Failed;
                    t.provision_pid = None;
                }
                Ok(())
            })?;
            bail!("step '{}' failed", step.label);
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
    println!("provisioning complete");
    Ok(())
}
