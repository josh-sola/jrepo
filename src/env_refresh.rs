use std::path::Path;

use anyhow::{Context, Result};

use crate::repo;
use crate::store;
use crate::tree;

/// Re-copies a repo's `copy` globs (e.g. `**/.env*`) from base into a tree,
/// overwriting whatever is already there — that's the whole job. It does
/// *not* regenerate values: `internal-cli config generate-env` is
/// monorepo-specific and needs AWS auth and network, so producing fresh env
/// values in base stays a manual step done separately.
pub fn refresh(root: &Path, selector: &str) -> Result<()> {
    let store = store::load(root)?;
    let target = store::resolve(&store.trees, selector)?;
    let repo = store.repos.get(&target.repo).with_context(|| {
        format!(
            "tree '{}' references unknown repo '{}'",
            target.name, target.repo
        )
    })?;

    let (shared, copy) = repo::parse_worktreeinclude(&repo.base)?;
    let copied = tree::copy_globs(&repo.base, &target.path, &copy, &shared)?;

    if copied.is_empty() {
        println!(
            "no files matched {:?} in {}; nothing copied",
            copy,
            repo.base.display()
        );
        return Ok(());
    }

    for relpath in &copied {
        println!("{relpath}");
    }
    println!(
        "copied {} file{} into {}",
        copied.len(),
        if copied.len() == 1 { "" } else { "s" },
        target.path.display()
    );
    Ok(())
}
