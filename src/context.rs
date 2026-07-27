use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::store;

/// Backs `hooks/session-context.sh`. Prints a few lines of plain-English
/// context for the resolved directory, or nothing if it isn't a registered
/// tree or base — the hook script treats empty output and any error the
/// same way, so this never needs to distinguish "not applicable" from
/// "couldn't tell".
pub fn session_context(root: &Path, cwd: Option<PathBuf>) -> Result<()> {
    let cwd = match cwd {
        Some(c) => c,
        None => std::env::current_dir()?,
    };
    let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
    let store = store::load(root)?;

    if let Some(tree) = store
        .trees
        .iter()
        .filter(|t| cwd.starts_with(&t.path))
        .max_by_key(|t| t.path.components().count())
    {
        let mut lines = vec![format!(
            "This directory is the wt tree \"{}\" (branch {}) of repo {}.",
            tree.name, tree.branch, tree.repo
        )];
        let plans = tree.path.join("plans");
        if plans.is_dir() {
            lines.push(format!(
                "Plans for this tree live at {} — read and write them there.",
                plans.display()
            ));
        }
        println!("{}", lines.join("\n"));
        return Ok(());
    }

    if let Some((repo_name, _)) = store.repos.iter().find(|(_, r)| cwd.starts_with(&r.base)) {
        println!(
            "This directory is {repo_name}'s base checkout. It stays on trunk; do not edit or \
             commit here. Run `wt new {repo_name} --name \"...\"` to start work in a disposable \
             tree."
        );
    }

    Ok(())
}
