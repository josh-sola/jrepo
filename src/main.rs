mod claude;
mod color;
mod context;
mod env_refresh;
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
        /// Open a `claude` session in the new tree once it exists.
        #[arg(long)]
        claude: bool,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Move uncommitted work out of base into a fresh tree.
    Adopt {
        repo: Option<String>,
        #[arg(long)]
        name: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long, value_delimiter = ',')]
        profile: Option<Vec<String>>,
        /// Open a `claude` session in the new tree once it exists.
        #[arg(long)]
        claude: bool,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Open a claude session in <repo>'s <name> tree, creating it if needed.
    /// Anything after `--` is passed straight to `claude`.
    Launch {
        repo: String,
        name: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long, value_delimiter = ',')]
        profile: Option<Vec<String>>,
        #[arg(last = true)]
        args: Vec<String>,
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
    /// Env-file maintenance for a tree.
    Env {
        #[command(subcommand)]
        action: EnvCommand,
    },
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

#[derive(Subcommand)]
enum EnvCommand {
    /// Re-copy the repo's `copy` globs from base into a tree, overwriting
    /// whatever is already there.
    Refresh { selector: String },
}

const PROVISION_WAIT_SECS: u64 = 600;

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
            claude,
            args,
        } => {
            let path = tree::new_tree(
                root,
                tree::NewOptions {
                    repo,
                    name,
                    branch,
                    profiles: profile,
                },
            )?;
            open_if_requested(root, &path, claude, &args)
        }
        Command::Adopt {
            repo,
            name,
            branch,
            profile,
            claude,
            args,
        } => {
            let path = tree::adopt(
                root,
                tree::AdoptOptions {
                    repo,
                    name,
                    branch,
                    profiles: profile,
                },
            )?;
            open_if_requested(root, &path, claude, &args)
        }
        Command::Launch {
            repo,
            name,
            branch,
            profile,
            args,
        } => cmd_launch(root, repo, name, branch, profile, &args),
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
        Command::Env { action } => match action {
            EnvCommand::Refresh { selector } => env_refresh::refresh(root, &selector),
        },
        Command::Claude { target, args } => claude::exec_claude(root, target, &args),
        Command::Provision { tree_id, profile } => provision::run(root, tree_id, profile),
        Command::SessionContext { path } => context::session_context(root, path),
    }
}

/// Blocks until provisioning finishes before handing the tree over: a
/// session that opens mid-install hits a half-built tree, and nobody
/// remembers to wait by hand. A failed install refuses to open at all.
fn open_if_requested(root: &Path, tree_path: &Path, claude: bool, args: &[String]) -> Result<()> {
    if !claude {
        return Ok(());
    }
    let id = store::load(root)?
        .trees
        .iter()
        .find(|t| t.path == tree_path)
        .map(|t| t.id)
        .with_context(|| format!("{} is not a registered tree", tree_path.display()))?;

    eprintln!("waiting for provisioning before opening a session...");
    wait_for_ready(root, id, PROVISION_WAIT_SECS).with_context(|| {
        format!(
            "not opening a session; the tree is still at {} — inspect it with `wt status`, then              `wt claude` into it once you know why",
            tree_path.display()
        )
    })?;

    eprintln!("provisioning finished; opening a claude session");
    claude::exec_at(tree_path, args)
}

fn cmd_launch(
    root: &Path,
    repo: String,
    name: String,
    branch: Option<String>,
    profile: Option<Vec<String>>,
    args: &[String],
) -> Result<()> {
    let existing = store::load(root)?
        .trees
        .iter()
        .find(|t| {
            t.repo == repo && (t.name == name || tree::slugify(&t.name) == tree::slugify(&name))
        })
        .map(|t| t.id);

    let id = match existing {
        Some(id) => id,
        None => {
            let path = tree::new_tree(
                root,
                tree::NewOptions {
                    repo: repo.clone(),
                    name: name.clone(),
                    branch,
                    profiles: profile,
                },
            )?;
            store::load(root)?
                .trees
                .iter()
                .find(|t| t.path == path)
                .map(|t| t.id)
                .with_context(|| format!("{} is not a registered tree", path.display()))?
        }
    };

    eprintln!("waiting for provisioning before opening a session...");
    let tree_path = wait_for_ready(root, id, PROVISION_WAIT_SECS).with_context(|| {
        format!(
            "not opening a session; inspect '{name}' with `wt status`, then `wt claude` into it \
             once you know why"
        )
    })?;

    eprintln!("provisioning finished; opening a claude session");
    let (color, hex) = color::pick(&repo, &name);
    color::set_background(hex);
    claude::exec_at(&tree_path, &launch_args(&name, args, color))
}

/// Claude takes the color as a slash-command prompt: the `--agent-color`
/// launch flag does not set the prompt-bar color.
fn launch_args(name: &str, passthrough: &[String], color: &str) -> Vec<String> {
    let mut args = vec!["-n".to_string(), name.to_string()];
    args.extend(passthrough.iter().cloned());
    args.push(format!("/color {color}"));
    args
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

    let ids: Vec<String> = rows
        .iter()
        .map(|(t, _)| unique_id_prefix(&t.id, &store.trees))
        .collect();
    let w = |header: &str, vals: &mut dyn Iterator<Item = usize>| {
        vals.chain(std::iter::once(header.len())).max().unwrap_or(0)
    };
    let name_w = w(
        "NAME",
        &mut rows.iter().map(|(t, _)| t.name.chars().count()),
    );
    let repo_w = w(
        "REPO",
        &mut rows.iter().map(|(t, _)| t.repo.chars().count()),
    );
    let branch_w = w(
        "BRANCH",
        &mut rows.iter().map(|(t, _)| t.branch.chars().count()),
    );
    let state_w = w(
        "STATE",
        &mut rows.iter().map(|(t, _)| state_str(t.state).len()),
    );
    let id_w = w("UUID", &mut ids.iter().map(String::len));

    println!(
        "{:<name_w$} {:<repo_w$} {:<branch_w$} {:<state_w$} {:<id_w$} DIRTY",
        "NAME", "REPO", "BRANCH", "STATE", "UUID"
    );
    for ((t, dirty), id) in rows.iter().zip(&ids) {
        println!(
            "{:<name_w$} {:<repo_w$} {:<branch_w$} {:<state_w$} {:<id_w$} {}",
            t.name,
            t.repo,
            t.branch,
            state_str(t.state),
            id,
            if *dirty { "dirty" } else { "" }
        );
    }
    Ok(())
}

/// A uuidv7 leads with a millisecond timestamp, so trees created minutes
/// apart share their first characters. This column doubles as a selector, so
/// it grows until it matches exactly one registered tree.
fn unique_id_prefix(id: &Uuid, all: &[store::Tree]) -> String {
    let full = id.to_string();
    for len in 8..full.len() {
        let candidate = &full[..len];
        let matches = all
            .iter()
            .filter(|t| t.id.to_string().starts_with(candidate))
            .count();
        if matches <= 1 {
            return candidate.to_string();
        }
    }
    full
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

    let path = wait_for_ready(root, id, timeout_secs)?;
    println!("{}", path.display());
    Ok(())
}

/// Progress goes to stderr so a caller reading stdout for the tree path is
/// unaffected. Returns the tree's path once it is ready.
fn wait_for_ready(root: &Path, id: Uuid, timeout_secs: u64) -> Result<PathBuf> {
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
            store::TreeState::Ready => return Ok(tree.path.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;
    use store::{Tree, TreeState};

    fn tree(id: &str) -> Tree {
        Tree {
            id: id.parse().unwrap(),
            repo: "monorepo".into(),
            name: "t".into(),
            branch: "josh/t".into(),
            path: PathBuf::from("/tmp/t"),
            created: Utc::now(),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
        }
    }

    #[test]
    fn launch_args_orders_name_flag_passthrough_then_color_last() {
        let args = launch_args(
            "fix login",
            &["--model".to_string(), "opus".to_string()],
            "blue",
        );
        assert_eq!(
            args,
            vec!["-n", "fix login", "--model", "opus", "/color blue"]
        );
    }

    #[test]
    fn id_prefix_grows_past_a_shared_uuidv7_timestamp() {
        let a = tree("019fa4ef-6669-7f32-a29c-a459aee6716b");
        let b = tree("019fa4ef-e6e2-78c2-977a-f55f5f00ab25");
        let all = vec![a.clone(), b.clone()];

        assert_eq!(unique_id_prefix(&a.id, &all), "019fa4ef-6");
        assert_eq!(unique_id_prefix(&b.id, &all), "019fa4ef-e");
        assert_eq!(
            unique_id_prefix(&a.id, std::slice::from_ref(&a)),
            "019fa4ef"
        );
    }
}
