use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::store::{self, Store};

/// Execs `claude` with cwd set to the resolved target, replacing this
/// process so the terminal, signals, and exit code pass straight through.
pub fn exec_claude(root: &Path, target: Option<String>, args: &[String]) -> Result<()> {
    let store = store::load(root)?;
    let (cwd, is_base) = resolve_target(&store, target)?;

    if is_base {
        eprintln!("base is for reading; run `wt new <repo> --name \"...\"` to start work instead");
    }

    let err = Command::new("claude").current_dir(&cwd).args(args).exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        bail!("`claude` is not on PATH");
    }
    Err(err).context("exec'ing claude")
}

/// Returns the resolved directory and whether it is a repo's base rather
/// than a tree.
fn resolve_target(store: &Store, target: Option<String>) -> Result<(PathBuf, bool)> {
    match target {
        Some(sel) => {
            if let Some(repo) = store.repos.get(&sel) {
                return Ok((repo.base.clone(), true));
            }
            let tree = store::resolve(&store.trees, &sel)?;
            Ok((tree.path.clone(), false))
        }
        None => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);

            if let Some(tree) = store
                .trees
                .iter()
                .filter(|t| cwd.starts_with(&t.path))
                .max_by_key(|t| t.path.components().count())
            {
                return Ok((tree.path.clone(), false));
            }
            if let Some((_, repo)) = store.repos.iter().find(|(_, r)| cwd.starts_with(&r.base)) {
                return Ok((repo.base.clone(), true));
            }
            bail!(
                "current directory is not a registered tree or base; pass a selector or repo name"
            );
        }
    }
}
