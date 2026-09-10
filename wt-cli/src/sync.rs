use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::config;
use crate::git;
use crate::store::{self, Repo};

/// Fetches every registered repo (or just `repo_filter`) and fast-forwards
/// its trunk when safe. Refreshing and topping up hot spares happens after a
/// successful sync. A repo that fails is reported without aborting the rest.
pub fn sync(root: &Path, config_path: &Path, repo_filter: Option<String>) -> Result<()> {
    let store = store::load(root)?;
    let config = config::load(config_path)?;
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
            .map(|(name, repo)| (name.clone(), repo.clone()))
            .collect(),
    };

    if repos.is_empty() {
        println!("no repos registered");
        return Ok(());
    }

    let mut had_failure = false;
    for (name, repo) in &repos {
        let repo_config = match config::repo(&config, name) {
            Ok(config) => config,
            Err(e) => {
                had_failure = true;
                println!("{name}: {e:#}");
                continue;
            }
        };
        match sync_one(root, name, repo, repo_config) {
            Ok(line) => {
                println!("{name}: {line}");
                if let Err(e) = crate::spare::refresh(root, config_path, Some(name)) {
                    println!("{name}: hot spare refresh failed: {e:#}");
                }
                if let Err(e) = crate::spare::top_up(root, config_path, Some(name)) {
                    println!("{name}: hot spare top-up failed: {e:#}");
                }
            }
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

/// The base's state, in the order it must be checked: a base with its own
/// uncommitted changes always blocks, before anything submodule-related is
/// even considered.
#[derive(Debug)]
enum BaseState {
    Clean,
    /// Stale or uninitialized submodule paths, each confirmed to hold
    /// nothing uncommitted of its own — safe to move.
    Repairable(Vec<String>),
    /// Human-readable reason naming what is in the way.
    Blocked(String),
}

fn repaired_note(paths: &[String]) -> String {
    format!(
        "repaired {} stale submodule pointer{}",
        paths.len(),
        if paths.len() == 1 { "" } else { "s" }
    )
}

/// A stale gitlink, a submodule with uncommitted work of its own, and the
/// base's own uncommitted changes all look identical to a plain `git status
/// --porcelain`. Only the first is safe to repair automatically; the other
/// two must still block a fast-forward exactly as an unfiltered dirty check
/// always has.
fn classify_base(base: &Path) -> Result<BaseState> {
    let own_changes = git::status_porcelain_filtered(base, git::SubmoduleFilter::All)?;
    if !own_changes.is_empty() {
        return Ok(BaseState::Blocked(format!(
            "dirty, refusing to fast-forward ({}): {}",
            own_changes.len(),
            own_changes.join("; ")
        )));
    }

    let submodules = git::submodule_status(base)?;
    if let Some(conflicted) = submodules
        .iter()
        .find(|s| matches!(s.state, git::SubmoduleState::Conflicted))
    {
        return Ok(BaseState::Blocked(format!(
            "submodule '{}' has unresolved merge conflicts, refusing to fast-forward",
            conflicted.path
        )));
    }

    let candidates: Vec<&git::SubmoduleEntry> = submodules
        .iter()
        .filter(|s| {
            matches!(
                s.state,
                git::SubmoduleState::StalePointer | git::SubmoduleState::Uninitialized
            )
        })
        .collect();

    if candidates.is_empty() {
        let plain = git::status_porcelain(base)?;
        if !plain.is_empty() {
            return Ok(BaseState::Blocked(format!(
                "dirty, refusing to fast-forward ({}): {}",
                plain.len(),
                plain.join("; ")
            )));
        }
        return Ok(BaseState::Clean);
    }

    for entry in &candidates {
        if matches!(entry.state, git::SubmoduleState::Uninitialized) {
            continue;
        }
        let dirty = git::status_porcelain(&base.join(&entry.path))?;
        if !dirty.is_empty() {
            return Ok(BaseState::Blocked(format!(
                "submodule '{}' has uncommitted changes, refusing to move it ({}): {}",
                entry.path,
                dirty.len(),
                dirty.join("; ")
            )));
        }
    }

    Ok(BaseState::Repairable(
        candidates.into_iter().map(|e| e.path.clone()).collect(),
    ))
}

fn sync_one(
    root: &Path,
    name: &str,
    repo: &Repo,
    repo_config: &config::RepoConfig,
) -> Result<String> {
    let trunk_ref = format!("origin/{}", repo_config.trunk);
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

    let repair_note = match classify_base(&repo.base)? {
        BaseState::Blocked(reason) => bail!(reason),
        BaseState::Repairable(paths) => {
            git::submodule_update_recursive(&repo.base)
                .with_context(|| format!("repairing {} stale submodule pointer(s)", paths.len()))?;
            Some(repaired_note(&paths))
        }
        BaseState::Clean => None,
    };
    let prefixed = |rest: String| match &repair_note {
        Some(note) => format!("{note}; {rest}"),
        None => rest,
    };

    let branch = git::current_branch(&repo.base)?;
    if branch != repo_config.trunk {
        return Ok(prefixed(format!(
            "{fetch_desc}; on branch '{branch}', not '{}' — skipping fast-forward",
            repo_config.trunk
        )));
    }

    let head = git::rev_parse(&repo.base, "HEAD").context("resolving HEAD")?;
    if head == after {
        return Ok(prefixed(format!("{fetch_desc}; trunk unchanged")));
    }

    git::merge_ff_only(&repo.base, &trunk_ref)?;
    let mut line = prefixed(format!(
        "{fetch_desc}; fast-forwarded {} to {}",
        repo_config.trunk,
        &after[..after.len().min(7)]
    ));

    // Non-fatal: the fast-forward already succeeded, so a submodule that
    // can't be repaired afterward is a warning on the status line, not a
    // failed sync.
    match classify_base(&repo.base) {
        Ok(BaseState::Repairable(paths)) => match git::submodule_update_recursive(&repo.base) {
            Ok(()) => line.push_str(&format!("; {} after fast-forward", repaired_note(&paths))),
            Err(e) => line.push_str(&format!(
                "; warning: fast-forward left {} submodule pointer(s) stale and the repair \
                 failed: {e:#}",
                paths.len()
            )),
        },
        Ok(BaseState::Blocked(reason)) => {
            line.push_str(&format!(
                "; warning: fast-forward left a submodule that needs attention: {reason}"
            ));
        }
        Ok(BaseState::Clean) => {}
        Err(e) => line.push_str(&format!(
            "; warning: could not verify submodule state after fast-forward: {e:#}"
        )),
    }

    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use uuid::Uuid;

    fn git(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn submodule_fixture() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("wt-sync-test-{}", Uuid::now_v7()));
        let sub = root.join("sub");
        let base = root.join("base");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(&base).unwrap();
        for path in [&sub, &base] {
            git(path, &["init", "-q", "-b", "main"]);
            git(path, &["config", "user.email", "test@example.com"]);
            git(path, &["config", "user.name", "Test"]);
        }
        fs::write(sub.join("file"), "one\n").unwrap();
        git(&sub, &["add", "."]);
        git(&sub, &["commit", "-qm", "initial"]);
        fs::write(base.join("file"), "one\n").unwrap();
        git(&base, &["add", "."]);
        git(&base, &["commit", "-qm", "initial"]);
        git(
            &base,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                sub.to_str().unwrap(),
                "sub",
            ],
        );
        git(&base, &["commit", "-qm", "add submodule"]);
        (base, sub)
    }

    #[test]
    fn classify_base_is_clean_when_nothing_is_dirty() {
        let (base, _) = submodule_fixture();
        assert!(matches!(classify_base(&base).unwrap(), BaseState::Clean));
        fs::remove_dir_all(base.parent().unwrap()).ok();
    }

    #[test]
    fn classify_base_repairs_a_stale_clean_submodule() {
        let (base, _) = submodule_fixture();
        fs::write(base.join("sub/file"), "two\n").unwrap();
        git(&base.join("sub"), &["commit", "-aqm", "advance"]);
        assert!(matches!(
            classify_base(&base).unwrap(),
            BaseState::Repairable(_)
        ));
        fs::remove_dir_all(base.parent().unwrap()).ok();
    }

    #[test]
    fn classify_base_blocks_uncommitted_work() {
        let (base, _) = submodule_fixture();
        fs::write(base.join("file"), "changed\n").unwrap();
        assert!(matches!(
            classify_base(&base).unwrap(),
            BaseState::Blocked(_)
        ));
        fs::remove_dir_all(base.parent().unwrap()).ok();
    }
}
