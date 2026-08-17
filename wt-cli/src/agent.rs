use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::store::{self, Store};

const BRIDGE_READY_TIMEOUT: Duration = Duration::from_secs(2);

const CODEX_ADMIN_COMMANDS: &[&str] = &[
    "a",
    "apply",
    "app",
    "app-server",
    "archive",
    "cloud",
    "completion",
    "delete",
    "debug",
    "doctor",
    "e",
    "exec",
    "exec-server",
    "features",
    "goal",
    "help",
    "login",
    "logout",
    "mcp",
    "mcp-server",
    "plugin",
    "remote-control",
    "review",
    "sandbox",
    "tasks",
    "unarchive",
    "update",
    "version",
];

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

pub fn exec_launch_codex(
    cwd: &Path,
    args: &[String],
    env: &[(&str, &str)],
    planter_enabled: bool,
) -> Result<()> {
    if !planter_enabled || !interactive_codex_args(args) {
        return exec_at(Agent::Codex, cwd, args, &[]);
    }

    let mut bridge = match start_bridge(cwd, env) {
        Ok(bridge) => bridge,
        Err(reason) => {
            eprintln!("wt: planter bridge unavailable ({reason}); starting Codex directly");
            return exec_at(Agent::Codex, cwd, args, &[]);
        }
    };

    let endpoint = match bridge_endpoint(&mut bridge) {
        Ok(endpoint) => endpoint,
        Err(reason) => {
            stop_bridge(&mut bridge);
            eprintln!("wt: planter bridge unavailable ({reason}); starting Codex directly");
            return exec_at(Agent::Codex, cwd, args, &[]);
        }
    };

    let mut command = Command::new(Agent::Codex.executable());
    command
        .current_dir(cwd)
        .arg("--remote")
        .arg(endpoint)
        .args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let err = command.exec();
    stop_bridge(&mut bridge);
    if err.kind() == std::io::ErrorKind::NotFound {
        bail!("`codex` is not on PATH; install it, then run `wt codex`");
    }
    Err(err).context("exec'ing codex through planter bridge")
}

pub(crate) fn interactive_codex_args(args: &[String]) -> bool {
    if args
        .iter()
        .any(|arg| arg == "--remote" || arg.starts_with("--remote="))
    {
        return false;
    }

    !args.iter().any(|arg| {
        matches!(arg.as_str(), "-h" | "--help" | "-V" | "--version")
            || CODEX_ADMIN_COMMANDS.contains(&arg.as_str())
    })
}

fn start_bridge(cwd: &Path, env: &[(&str, &str)]) -> std::result::Result<Child, String> {
    let mut command = Command::new("planter-codex-bridge");
    command
        .current_dir(cwd)
        .arg("--owner-pid")
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        // The bridge owns its child; keeping it in a separate group lets a
        // failed pre-exec path clean the whole bridge tree without touching wt.
        .process_group(0);
    for (key, value) in env {
        command.env(key, value);
    }
    command.spawn().map_err(|error| error.to_string())
}

fn bridge_endpoint(bridge: &mut Child) -> std::result::Result<String, String> {
    let stdout = bridge
        .stdout
        .take()
        .ok_or_else(|| "bridge stdout was unavailable".to_string())?;
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(result.map(|_| line));
    });

    let line = match rx.recv_timeout(BRIDGE_READY_TIMEOUT) {
        Ok(Ok(line)) => line,
        Ok(Err(error)) => return Err(format!("could not read readiness: {error}")),
        Err(mpsc::RecvTimeoutError::Timeout) => return Err("readiness timed out".to_string()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err("readiness reader stopped".to_string());
        }
    };
    valid_endpoint(line.trim()).ok_or_else(|| "invalid readiness endpoint".to_string())
}

fn valid_endpoint(value: &str) -> Option<String> {
    let path = value.strip_prefix("unix://")?;
    Path::new(path).is_absolute().then(|| value.to_string())
}

fn stop_bridge(bridge: &mut Child) {
    let pid = bridge.id();
    let _ = Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        if bridge.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = Command::new("kill")
        .args(["-KILL", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = bridge.wait();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn interactive_codex_accepts_default_resume_and_fork() {
        assert!(interactive_codex_args(&[]));
        assert!(interactive_codex_args(&args(&["resume"])));
        assert!(interactive_codex_args(&args(&["fork", "thr_123"])));
    }

    #[test]
    fn interactive_codex_bypasses_explicit_remote_and_admin_commands() {
        assert!(!interactive_codex_args(&args(&[
            "--remote",
            "unix:///tmp/codex.sock"
        ])));
        assert!(!interactive_codex_args(&args(&[
            "--remote=unix:///tmp/codex.sock"
        ])));
        assert!(!interactive_codex_args(&args(&["exec", "--json"])));
        assert!(!interactive_codex_args(&args(&[
            "--model", "gpt-x", "exec"
        ])));
        assert!(!interactive_codex_args(&args(&["plugin", "list"])));
        assert!(!interactive_codex_args(&args(&["e", "--json"])));
        assert!(!interactive_codex_args(&args(&["app-server"])));
        assert!(!interactive_codex_args(&args(&["--help"])));
        assert!(!interactive_codex_args(&args(&["-V"])));
    }

    #[test]
    fn valid_endpoint_requires_an_absolute_unix_path() {
        assert_eq!(
            valid_endpoint("unix:///tmp/codex.sock"),
            Some("unix:///tmp/codex.sock".into())
        );
        assert_eq!(valid_endpoint("unix://relative.sock"), None);
        assert_eq!(valid_endpoint("ws://localhost"), None);
    }
}
