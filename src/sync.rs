use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::git;
use crate::store::{self, Repo};

/// Fetches every registered repo (or just `repo_filter`) and fast-forwards
/// its trunk when safe. Never destructive: a repo that fails to sync is
/// reported and skipped rather than aborting the rest.
pub fn sync(root: &Path, repo_filter: Option<String>) -> Result<()> {
    let store = store::load(root)?;
    let repos: Vec<(String, Repo)> = match repo_filter {
        Some(name) => {
            let repo = store
                .repos
                .get(&name)
                .cloned()
                .with_context(|| format!("unknown repo '{name}'"))?;
            vec![(name, repo)]
        }
        None => store
            .repos
            .iter()
            .map(|(n, r)| (n.clone(), r.clone()))
            .collect(),
    };

    if repos.is_empty() {
        println!("no repos registered");
        return Ok(());
    }

    let mut had_failure = false;
    for (name, repo) in &repos {
        match sync_one(root, name, repo) {
            Ok(line) => println!("{name}: {line}"),
            Err(e) => {
                had_failure = true;
                println!("{name}: {e:#}");
            }
        }
    }

    if had_failure {
        bail!("one or more repos failed to sync");
    }
    Ok(())
}

fn sync_one(root: &Path, name: &str, repo: &Repo) -> Result<String> {
    let trunk_ref = format!("origin/{}", repo.trunk);
    let before = git::rev_parse(&repo.base, &trunk_ref).ok();

    git::fetch_prune(&repo.base)?;
    store::with_store_lock(root, |s| {
        if let Some(r) = s.repos.get_mut(name) {
            r.last_fetch = Some(Utc::now());
        }
        Ok(())
    })?;

    let after = git::rev_parse(&repo.base, &trunk_ref)
        .with_context(|| format!("resolving {trunk_ref} after fetch"))?;
    let fetch_desc = if before.as_deref() == Some(after.as_str()) {
        "up to date"
    } else {
        "fetched new commits"
    };

    let dirty = git::status_porcelain(&repo.base)?;
    if !dirty.is_empty() {
        bail!(
            "dirty, refusing to fast-forward ({}): {}",
            dirty.len(),
            dirty.join("; ")
        );
    }

    let branch = git::current_branch(&repo.base)?;
    if branch != repo.trunk {
        return Ok(format!(
            "{fetch_desc}; on branch '{branch}', not '{}' — skipping fast-forward",
            repo.trunk
        ));
    }

    let head = git::rev_parse(&repo.base, "HEAD").context("resolving HEAD")?;
    if head == after {
        return Ok(format!("{fetch_desc}; trunk unchanged"));
    }

    git::merge_ff_only(&repo.base, &trunk_ref)?;
    Ok(format!(
        "{fetch_desc}; fast-forwarded {} to {}",
        repo.trunk,
        &after[..after.len().min(7)]
    ))
}
