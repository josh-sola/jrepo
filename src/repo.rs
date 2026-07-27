use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::git;
use crate::store::{self, Repo, Step};

const PYTHON_PROJECTS: &[&str] = &[
    "python/datahub",
    "python/dspy-worker",
    "python/data-processing-worker",
    "python/scripts",
];

const EXCLUDE_MARKER: &str = "# wt-cli: shared symlinks (managed, do not edit by hand)";

pub struct InitOptions {
    pub name: String,
    pub adopt_path: PathBuf,
    pub branch_prefix: String,
}

pub fn init(root: &Path, opts: InitOptions) -> Result<()> {
    let base = fs::canonicalize(&opts.adopt_path)
        .with_context(|| format!("resolving {}", opts.adopt_path.display()))?;

    if !git::is_git_repo(&base)? {
        bail!("{} is not a git repository", base.display());
    }
    if !git::has_origin_remote(&base)? {
        bail!("{} has no 'origin' remote", base.display());
    }
    let trunk = git::trunk_branch(&base);

    let repo_dir = root.join(&opts.name);
    fs::create_dir_all(repo_dir.join("trees"))?;
    fs::create_dir_all(repo_dir.join("shared"))?;
    fs::create_dir_all(repo_dir.join("cache").join("cargo-target"))?;

    link_base(&repo_dir.join("base"), &base)?;

    let (shared, copy) = parse_worktreeinclude(&base)?;

    for relpath in &shared {
        seed_shared_dir(&base, &repo_dir.join("shared"), relpath)?;
    }
    exclude_shared_paths(&base, &shared)?;

    let steps = detect_steps(&base);

    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        repo_dir
            .join("cache")
            .join("cargo-target")
            .to_string_lossy()
            .to_string(),
    );

    store::with_store_lock(root, |store| {
        let last_fetch = store.repos.get(&opts.name).and_then(|r| r.last_fetch);
        store.repos.insert(
            opts.name.clone(),
            Repo {
                base: base.clone(),
                trunk,
                branch_prefix: opts.branch_prefix.clone(),
                last_fetch,
                shared,
                copy,
                env,
                steps,
            },
        );
        Ok(())
    })?;

    Ok(())
}

/// Errors when `base` already exists and points somewhere else — adopting a
/// second path under the same repo name would silently orphan the first.
fn link_base(link: &Path, target: &Path) -> Result<()> {
    match fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let existing = fs::read_link(link)?;
            if existing != target {
                bail!(
                    "{} already links to {}, not {}",
                    link.display(),
                    existing.display(),
                    target.display()
                );
            }
            Ok(())
        }
        Ok(_) => bail!("{} exists and is not a symlink", link.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            symlink(target, link).with_context(|| format!("symlinking {}", link.display()))
        }
        Err(e) => Err(e).with_context(|| format!("checking {}", link.display())),
    }
}

const GLOB_CHARS: &[char] = &['*', '?', '['];

/// `.worktreeinclude` entries split by shape: a trailing `/` names a
/// directory to symlink from `shared/`; a glob names files to copy fresh
/// into every tree. Entries absent from base are dropped rather than
/// registered — there is nothing to link or copy.
fn parse_worktreeinclude(base: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let path = base.join(".worktreeinclude");
    let contents = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), vec!["**/.env*".to_string()]));
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };

    let mut shared = Vec::new();
    let mut copy = Vec::new();
    for line in contents.lines() {
        let entry = line.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        if entry.contains(GLOB_CHARS) {
            copy.push(entry.to_string());
        } else if let Some(relpath) = entry.strip_suffix('/')
            && base.join(relpath).exists()
        {
            shared.push(relpath.to_string());
        }
    }
    Ok((shared, copy))
}

/// Copies, never moves: base must stay byte-identical.
fn seed_shared_dir(base: &Path, shared_root: &Path, relpath: &str) -> Result<()> {
    let dst = shared_root.join(relpath);
    fs::create_dir_all(&dst).with_context(|| format!("creating {}", dst.display()))?;

    let is_empty = fs::read_dir(&dst)?.next().is_none();
    if !is_empty {
        return Ok(());
    }
    let src = base.join(relpath);
    let has_content = fs::read_dir(&src)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if !has_content {
        return Ok(());
    }

    let src_contents = format!("{}/.", src.display());
    let cloned = Command::new("cp")
        .args(["-c", "-R", &src_contents, &dst.to_string_lossy()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if cloned {
        return Ok(());
    }
    let status = Command::new("cp")
        .args(["-R", &src_contents, &dst.to_string_lossy()])
        .status()
        .with_context(|| format!("copying {} into {}", src.display(), dst.display()))?;
    if !status.success() {
        bail!("cp -R {} {} failed", src.display(), dst.display());
    }
    Ok(())
}

/// `info/exclude` lives in the common git dir, so it applies to every
/// worktree of the clone without ever being tracked or committed — unlike
/// `.gitignore`, nothing here risks landing in a commit.
fn exclude_shared_paths(base: &Path, shared: &[String]) -> Result<()> {
    if shared.is_empty() {
        return Ok(());
    }
    let common = git::common_dir(base)?;
    let exclude_path = common.join("info").join("exclude");
    fs::create_dir_all(exclude_path.parent().unwrap())?;
    let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    let mut lines: Vec<&str> = existing.lines().collect();

    let mut to_add = Vec::new();
    if !lines.contains(&EXCLUDE_MARKER) {
        to_add.push(EXCLUDE_MARKER);
    }
    for relpath in shared {
        if !lines.contains(&relpath.as_str()) {
            to_add.push(relpath.as_str());
        }
    }
    if to_add.is_empty() {
        return Ok(());
    }
    lines.extend(to_add.iter().copied());
    let mut out = lines.join("\n");
    out.push('\n');

    // A local checkout's existing excludes matter; write-then-rename avoids
    // losing them to a truncate-in-place if the write is interrupted.
    let tmp = exclude_path.with_extension("tmp");
    fs::write(&tmp, out).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &exclude_path)
        .with_context(|| format!("renaming {} into place", tmp.display()))
}

fn detect_steps(base: &Path) -> Vec<Step> {
    let mut steps = Vec::new();

    if base.join(".gitmodules").exists() {
        steps.push(Step {
            label: "submodules".to_string(),
            profile: "node".to_string(),
            cwd: ".".to_string(),
            cmd: vec![
                "git".to_string(),
                "submodule".to_string(),
                "update".to_string(),
                "--init".to_string(),
                "--recursive".to_string(),
            ],
        });
    }

    if base.join("pnpm-lock.yaml").exists() {
        steps.push(Step {
            label: "pnpm-install".to_string(),
            profile: "node".to_string(),
            cwd: ".".to_string(),
            cmd: vec![
                "pnpm".to_string(),
                "install".to_string(),
                "--frozen-lockfile".to_string(),
            ],
        });
        steps.push(Step {
            label: "pnpm-build-packages".to_string(),
            profile: "node".to_string(),
            cwd: ".".to_string(),
            cmd: vec!["pnpm".to_string(), "build:packages".to_string()],
        });
    }

    if base.join("python").join("pyproject.toml").exists() {
        steps.push(Step {
            label: "uv-sync-python".to_string(),
            profile: "python".to_string(),
            cwd: "python".to_string(),
            cmd: vec![
                "uv".to_string(),
                "sync".to_string(),
                "--all-packages".to_string(),
            ],
        });
        for project in PYTHON_PROJECTS {
            if base.join(project).exists() {
                let label = format!("uv-sync-{}", project.rsplit('/').next().unwrap());
                steps.push(Step {
                    label,
                    profile: "python".to_string(),
                    cwd: project.to_string(),
                    cmd: vec!["uv".to_string(), "sync".to_string()],
                });
            }
        }
    }

    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wt-repo-test-{label}-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn worktreeinclude_splits_shared_and_copy_and_skips_missing() {
        let base = temp_dir("wti");
        fs::create_dir_all(base.join("plans")).unwrap();
        fs::write(
            base.join(".worktreeinclude"),
            "# comment\n\nplans/\nlocal/\n**/.env*\n",
        )
        .unwrap();

        let (shared, copy) = parse_worktreeinclude(&base).unwrap();
        assert_eq!(shared, vec!["plans".to_string()]);
        assert_eq!(copy, vec!["**/.env*".to_string()]);
    }

    #[test]
    fn missing_worktreeinclude_defaults_to_env_copy_only() {
        let base = temp_dir("noWti");
        let (shared, copy) = parse_worktreeinclude(&base).unwrap();
        assert!(shared.is_empty());
        assert_eq!(copy, vec!["**/.env*".to_string()]);
    }

    #[test]
    fn seed_shared_dir_copies_base_content_without_moving_it() {
        let base = temp_dir("seed-base");
        let shared_root = temp_dir("seed-shared");
        fs::create_dir_all(base.join("plans")).unwrap();
        fs::write(base.join("plans").join("a.md"), "hello").unwrap();

        seed_shared_dir(&base, &shared_root, "plans").unwrap();

        assert_eq!(
            fs::read_to_string(shared_root.join("plans").join("a.md")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(base.join("plans").join("a.md")).unwrap(),
            "hello"
        );
    }
}
