mod claude;
mod context;
mod git;
mod proc;
mod provision;
mod repo;
mod store;
mod sync;
mod tree;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "wt", about = "Enriched worktree tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Register an existing clone as a repo's base.
    Init {
        name: String,
        #[arg(long)]
        adopt: PathBuf,
        /// Prefix applied to branch names `wt new` generates for this repo.
        #[arg(long, default_value = "josh/")]
        branch_prefix: String,
    },
    /// Create a new worktree and provision it.
    New {
        repo: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long, value_delimiter = ',')]
        profile: Option<Vec<String>>,
    },
    /// List registered worktrees.
    Ls {
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print a worktree's absolute path.
    Path { selector: String },
    /// Print the worktree name for a path (statusline lookup).
    Name {
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Remove a worktree.
    Rm {
        selector: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        delete_branch: bool,
    },
    /// Show provisioning status; every non-ready tree if no selector.
    Status {
        selector: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Block until a tree is ready or failed.
    Wait {
        selector: Option<String>,
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Reap clean trees with no commits beyond origin/<trunk>.
    Gc {
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Reconcile the registry against git's worktree list.
    Doctor {
        #[arg(long)]
        fix: bool,
    },
    /// Fetch and fast-forward base's trunk; refuses if base is dirty.
    Sync { repo: Option<String> },
    /// Exec `claude` with cwd set to a tree, a repo's base, or the cwd's tree.
    /// Anything after `--` is passed straight to `claude`.
    Claude {
        target: Option<String>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Runs a tree's provisioning steps; spawned detached by `wt new`.
    #[command(name = "__provision", hide = true)]
    Provision {
        tree_id: Uuid,
        #[arg(long, value_delimiter = ',')]
        profile: Option<Vec<String>>,
    },
    /// Prints SessionStart/CwdChanged hook context; backs `hooks/session-context.sh`.
    #[command(name = "__session-context", hide = true)]
    SessionContext {
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = store::root_dir();
    match run(&root, cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &Path, command: Command) -> Result<()> {
    match command {
        Command::Init {
            name,
            adopt,
            branch_prefix,
        } => repo::init(
            root,
            repo::InitOptions {
                name,
                adopt_path: adopt,
                branch_prefix,
            },
        ),
        Command::New {
            repo,
            name,
            branch,
            profile,
        } => tree::new_tree(
            root,
            tree::NewOptions {
                repo,
                name,
                branch,
                profiles: profile,
            },
        )
        .map(|_| ()),
        Command::Ls { repo, json } => cmd_ls(root, repo, json),
        Command::Path { selector } => cmd_path(root, &selector),
        Command::Name { path } => cmd_name(root, path),
        Command::Rm {
            selector,
            force,
            delete_branch,
        } => tree::rm_tree(root, &selector, force, delete_branch),
        Command::Status { selector, json } => cmd_status(root, selector, json),
        Command::Wait { selector, timeout } => cmd_wait(root, selector, timeout),
        Command::Gc { repo, dry_run } => tree::gc(root, tree::GcOptions { repo, dry_run }),
        Command::Doctor { fix } => tree::doctor(root, tree::DoctorOptions { fix }),
        Command::Sync { repo } => sync::sync(root, repo),
        Command::Claude { target, args } => claude::exec_claude(root, target, &args),
        Command::Provision { tree_id, profile } => provision::run(root, tree_id, profile),
        Command::SessionContext { path } => context::session_context(root, path),
    }
}

fn cmd_path(root: &Path, selector: &str) -> Result<()> {
    let store = store::load(root)?;
    let tree = store::resolve(&store.trees, selector)?;
    println!("{}", tree.path.display());
    Ok(())
}

/// Never errors on an unresolved path — the statusline calls this every
/// few seconds and a bare tool must stay silent, not noisy, when the cwd
/// isn't a tree.
fn cmd_name(root: &Path, path: Option<PathBuf>) -> Result<()> {
    let target = match path {
        Some(p) => p,
        None => std::env::current_dir().context("reading current directory")?,
    };
    let target = std::fs::canonicalize(&target).unwrap_or(target);

    let store = store::load(root)?;
    let best = store
        .trees
        .iter()
        .filter(|t| target.starts_with(&t.path))
        .max_by_key(|t| t.path.components().count());

    if let Some(tree) = best {
        println!("{}", tree.name);
    }
    Ok(())
}

fn cmd_ls(root: &Path, repo_filter: Option<String>, json: bool) -> Result<()> {
    let store = store::load(root)?;
    let mut rows = Vec::new();
    for t in &store.trees {
        if let Some(ref r) = repo_filter
            && &t.repo != r
        {
            continue;
        }
        let dirty = git::is_dirty(&t.path).unwrap_or(false);
        rows.push((t, dirty));
    }

    if json {
        let entries: Vec<_> = rows
            .iter()
            .map(|(t, dirty)| {
                serde_json::json!({
                    "id": t.id,
                    "repo": t.repo,
                    "name": t.name,
                    "branch": t.branch,
                    "path": t.path,
                    "created": t.created,
                    "state": t.state,
                    "dirty": dirty,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    println!(
        "{:<24} {:<12} {:<28} {:<12} {:<8} DIRTY",
        "NAME", "REPO", "BRANCH", "STATE", "UUID"
    );
    for (t, dirty) in rows {
        let short_id = &t.id.to_string()[..8];
        println!(
            "{:<24} {:<12} {:<28} {:<12} {:<8} {}",
            t.name,
            t.repo,
            t.branch,
            state_str(t.state),
            short_id,
            if dirty { "dirty" } else { "" }
        );
    }
    Ok(())
}

fn state_str(state: store::TreeState) -> &'static str {
    match state {
        store::TreeState::Provisioning => "provisioning",
        store::TreeState::Ready => "ready",
        store::TreeState::Failed => "failed",
    }
}

fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

fn step_str(t: &store::Tree) -> String {
    match (t.step_index, t.step_total, &t.step_label) {
        (Some(i), Some(total), Some(label)) => format!("{i}/{total} {label}"),
        _ => "-".to_string(),
    }
}

/// A `provisioning` tree whose recorded pid is no longer alive is wedged,
/// not progressing — a killed child leaves the row in `provisioning`
/// forever otherwise. `None` (not yet recorded, or an older registry entry)
/// is not flagged: there is nothing to compare against.
fn provisioning_is_stale(t: &store::Tree) -> bool {
    t.state == store::TreeState::Provisioning
        && t.provision_pid.is_some_and(|pid| !proc::pid_alive(pid))
}

fn status_state_str(t: &store::Tree) -> String {
    if provisioning_is_stale(t) {
        format!("{} (stale)", state_str(t.state))
    } else {
        state_str(t.state).to_string()
    }
}

fn cmd_status(root: &Path, selector: Option<String>, json: bool) -> Result<()> {
    let store = store::load(root)?;
    let trees: Vec<&store::Tree> = match &selector {
        Some(sel) => vec![store::resolve(&store.trees, sel)?],
        None => store
            .trees
            .iter()
            .filter(|t| t.state != store::TreeState::Ready)
            .collect(),
    };

    if json {
        let entries: Vec<_> = trees
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "repo": t.repo,
                    "name": t.name,
                    "branch": t.branch,
                    "path": t.path,
                    "state": t.state,
                    "stale": provisioning_is_stale(t),
                    "stepLabel": t.step_label,
                    "stepIndex": t.step_index,
                    "stepTotal": t.step_total,
                    "logPath": t.log_path,
                    "elapsedSeconds": (Utc::now() - t.created).num_seconds().max(0),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if trees.is_empty() {
        println!("all trees are ready");
        return Ok(());
    }

    println!(
        "{:<24} {:<12} {:<21} {:<32} {:<8} LOG",
        "NAME", "REPO", "STATE", "STEP", "ELAPSED"
    );
    for t in trees {
        let elapsed = format_duration((Utc::now() - t.created).num_seconds());
        let log = t
            .log_path
            .as_ref()
            .map_or("-".to_string(), |p| p.display().to_string());
        println!(
            "{:<24} {:<12} {:<21} {:<32} {:<8} {}",
            t.name,
            t.repo,
            status_state_str(t),
            step_str(t),
            elapsed,
            log
        );
    }
    Ok(())
}

fn cmd_wait(root: &Path, selector: Option<String>, timeout_secs: u64) -> Result<()> {
    let id = {
        let store = store::load(root)?;
        match &selector {
            Some(sel) => store::resolve(&store.trees, sel)?.id,
            None => {
                let provisioning: Vec<_> = store
                    .trees
                    .iter()
                    .filter(|t| t.state == store::TreeState::Provisioning)
                    .collect();
                match provisioning.len() {
                    0 => bail!("no tree is provisioning"),
                    1 => provisioning[0].id,
                    _ => {
                        let names = provisioning
                            .iter()
                            .map(|t| format!("{} ({})", t.name, t.id))
                            .collect::<Vec<_>>()
                            .join(", ");
                        bail!("multiple trees are provisioning, pass a selector: {names}");
                    }
                }
            }
        }
    };

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut last_step: Option<String> = None;
    loop {
        let store = store::load(root)?;
        let tree = store
            .trees
            .iter()
            .find(|t| t.id == id)
            .with_context(|| format!("tree {id} is no longer registered"))?;

        match tree.state {
            store::TreeState::Ready => {
                println!("{}", tree.path.display());
                return Ok(());
            }
            store::TreeState::Failed => {
                let log = tree
                    .log_path
                    .as_ref()
                    .map_or("(no log)".to_string(), |p| p.display().to_string());
                bail!("provisioning failed for '{}'; see {log}", tree.name);
            }
            store::TreeState::Provisioning => {
                if tree.step_label != last_step {
                    eprintln!("{}", step_str(tree));
                    last_step = tree.step_label.clone();
                }
            }
        }

        if Instant::now() >= deadline {
            bail!(
                "timed out after {timeout_secs}s waiting for '{}'",
                tree.name
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
