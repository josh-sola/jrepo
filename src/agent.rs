use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::store::{self, Store};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Agent {
    Codex,
    Claude,
}

impl Agent {
    pub fn from_flags(codex: bool, claude: bool) -> Option<Self> {
        match (codex, claude) {
            (true, false) => Some(Self::Codex),
            (false, true) => Some(Self::Claude),
            (false, false) => None,
            (true, true) => unreachable!("clap rejects conflicting agent flags"),
        }
    }

    pub fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

pub struct LaunchTarget {
    pub cwd: PathBuf,
    pub is_base: bool,
}

/// Execs the selected agent with cwd set to the resolved target, replacing
/// this process so the terminal, signals, and exit code pass straight through.
pub fn exec_target(
    root: &Path,
    agent: Agent,
    target: Option<String>,
    args: &[String],
) -> Result<()> {
    let store = store::load(root)?;
    let target = resolve_target(&store, target)?;

    if target.is_base {
        eprintln!("base is for reading; run `wt new <repo> --name \"...\"` to start work instead");
    }

    exec_at(agent, &target.cwd, args, &[])
}

/// Only returns on failure: a successful `exec` replaces this process.
pub fn exec_at(agent: Agent, cwd: &Path, args: &[String], env: &[(&str, &str)]) -> Result<()> {
    let mut cmd = Command::new(agent.executable());
    cmd.current_dir(cwd).args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    let err = cmd.exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        bail!(
            "`{}` is not on PATH; install it, then run `wt {}`",
            agent.executable(),
            agent.executable()
        );
    }
    Err(err).with_context(|| format!("exec'ing {}", agent.executable()))
}

fn resolve_target(store: &Store, target: Option<String>) -> Result<LaunchTarget> {
    match target {
        Some(sel) => {
            if let Some(repo) = store.repos.get(&sel) {
                return Ok(LaunchTarget {
                    cwd: repo.base.clone(),
                    is_base: true,
                });
            }
            let tree = store::resolve(&store.trees, &sel)?;
            Ok(LaunchTarget {
                cwd: tree.path.clone(),
                is_base: false,
            })
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
                return Ok(LaunchTarget {
                    cwd: tree.path.clone(),
                    is_base: false,
                });
            }
            if let Some((_, repo)) = store.repos.iter().find(|(_, r)| cwd.starts_with(&r.base)) {
                return Ok(LaunchTarget {
                    cwd: repo.base.clone(),
                    is_base: true,
                });
            }
            bail!(
                "current directory is not a registered tree or base; pass a selector or repo name"
            );
        }
    }
}
