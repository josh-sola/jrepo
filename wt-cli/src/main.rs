mod agent;
mod color;
mod config;
mod context;
mod env_refresh;
mod features;
mod git;
mod graphite;
mod migrate;
mod pick;
mod planter;
mod proc;
mod provision;
mod repo;
mod restack;
mod spare;
mod stack;
mod store;
mod sync;
mod tab;
mod tree;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use uuid::Uuid;

use agent::Agent;

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
        /// Defaults to "josh/" for a fresh repo; ignored (with a warning)
        /// when the repo already has a config block.
        #[arg(long)]
        branch_prefix: Option<String>,
        /// Regenerate just the detected provisioning steps of an existing
        /// config block.
        #[arg(long)]
        redetect: bool,
    },
    /// Create a new worktree and provision it.
    New {
        repo: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        branch: Option<String>,
        /// Branch the new tree from here instead of `origin/<trunk>`: a `wt`
        /// tree selector (its live branch), a branch name, or a commit-ish.
        #[arg(long)]
        onto: Option<String>,
        #[arg(long, value_delimiter = ',')]
        profile: Option<Vec<String>>,
        /// Open a `codex` session in the new tree once it exists.
        #[arg(long, conflicts_with_all = ["claude", "claudex"])]
        codex: bool,
        /// Open a `claude` session in the new tree once it exists.
        #[arg(long, conflicts_with_all = ["codex", "claudex"])]
        claude: bool,
        /// Open a `claudex` session in the new tree once it exists.
        #[arg(long, conflicts_with_all = ["codex", "claude"])]
        claudex: bool,
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
        /// Open a `codex` session in the new tree once it exists.
        #[arg(long, conflicts_with_all = ["claude", "claudex"])]
        codex: bool,
        /// Open a `claude` session in the new tree once it exists.
        #[arg(long, conflicts_with_all = ["codex", "claudex"])]
        claude: bool,
        /// Open a `claudex` session in the new tree once it exists.
        #[arg(long, conflicts_with_all = ["codex", "claude"])]
        claudex: bool,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Open a Codex session in a worktree, creating it if needed. Pass
    /// --claude or --claudex to choose that agent instead. A
    /// worktree starting with `@` is a scratch session that opens in the
    /// repo's base instead, with nothing created. With no worktree given, an
    /// fzf picker lists the registered trees to choose from. Anything after
    /// `--` is passed straight to the selected agent.
    Launch {
        worktree: Option<String>,
        repo: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        /// Branch a newly created tree from here instead of
        /// `origin/<trunk>`: a `wt` tree selector (its live branch), a
        /// branch name, or a commit-ish. Only applies when `worktree` names
        /// a tree that doesn't exist yet.
        #[arg(long)]
        onto: Option<String>,
        #[arg(long, value_delimiter = ',')]
        profile: Option<Vec<String>>,
        /// Open Claude instead of Codex.
        #[arg(long, conflicts_with = "claudex")]
        claude: bool,
        /// Open Claudex instead of Codex.
        #[arg(long, conflicts_with = "claude")]
        claudex: bool,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// List registered worktrees.
    Ls {
        #[arg(long)]
        repo: Option<String>,
        /// Also show each repo's hot spare, hidden by default.
        #[arg(long)]
        all: bool,
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
        /// When the deleted branch has Graphite children, re-parent each
        /// one onto the deleted branch's own parent (trunk, if it has none)
        /// instead of refusing.
        #[arg(long)]
        reparent_children: bool,
    },
    /// Show provisioning status; every non-ready tree if no selector.
    Status {
        selector: Option<String>,
        /// Also show a non-ready hot spare, hidden by default.
        #[arg(long)]
        all: bool,
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
    Sync {
        repo: Option<String>,
        /// Also restack every stack in the repo that has branches in more
        /// than one worktree, walking bottom-up from wherever each branch
        /// lives. Never deletes a branch — that's `gt sync`'s job.
        #[arg(long)]
        stack: bool,
    },
    /// Restack a Graphite stack across every worktree that holds one of its
    /// branches, bottom-up. A selector resolves like `wt stack`'s: a
    /// worktree name/uuid first, then a branch name; with none, the stack
    /// containing the current tree's (or base's) checked-out branch.
    Restack {
        selector: Option<String>,
        /// Print the ordered plan and run nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the Graphite stack containing a branch, with `wt` identity —
    /// tree names instead of raw worktree paths. A selector is a worktree
    /// name/uuid first, then a branch name; with none, the stack containing
    /// the current tree's (or base's) checked-out branch.
    Stack {
        selector: Option<String>,
        #[arg(long)]
        json: bool,
        /// Every stack in the repo, not just the one containing `selector`.
        #[arg(long)]
        all: bool,
        /// Show merged and closed branches too; hidden by default.
        #[arg(long)]
        all_branches: bool,
    },
    /// Env-file maintenance for a tree.
    Env {
        #[command(subcommand)]
        action: EnvCommand,
    },
    /// Show or manage each repo's hot spare — a pre-provisioned worktree
    /// `wt new` claims instead of building one from cold.
    Spare {
        #[command(subcommand)]
        action: Option<SpareCommand>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Exec `claude` with cwd set to a tree, a repo's base, or the cwd's tree.
    /// Anything after `--` is passed straight to `claude`.
    Claude {
        target: Option<String>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Exec `codex` with cwd set to a tree, a repo's base, or the cwd's tree.
    /// Anything after `--` is passed straight to `codex`.
    Codex {
        target: Option<String>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Exec `claudex` with cwd set to a tree, a repo's base, or the cwd's tree.
    /// Anything after `--` is passed straight to `claudex`.
    Claudex {
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
    /// Builds or refreshes one hot spare; spawned detached by `wt new`,
    /// `wt sync`, and `wt spare refresh`.
    #[command(name = "__spare", hide = true)]
    SpareInternal {
        #[command(subcommand)]
        action: SpareInternalCommand,
    },
    /// Prints SessionStart/CwdChanged hook context; backs `hooks/session-context.sh`.
    #[command(name = "__session-context", hide = true)]
    SessionContext {
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Prints this tab's 1-based rank among the Ghostty tabs in the
    /// caller's own window that are running Claude Code.
    #[command(name = "__tab-index", hide = true)]
    TabIndex,
    /// Prints a tree's details for the `wt launch` fzf preview window.
    #[command(name = "__launch-preview", hide = true)]
    LaunchPreview { selector: String },
}

#[derive(Subcommand)]
enum EnvCommand {
    /// Re-copy the repo's `copy` globs from base into a tree, overwriting
    /// whatever is already there.
    Refresh { selector: String },
}

#[derive(Subcommand)]
enum SpareCommand {
    /// Force a refresh now instead of waiting for `wt sync`.
    Refresh {
        #[arg(long)]
        repo: Option<String>,
    },
    /// Remove a repo's spare and turn spares off for it, so `wt sync`/`wt
    /// new` don't immediately rebuild one.
    Drop {
        #[arg(long)]
        repo: Option<String>,
    },
}

#[derive(Subcommand)]
enum SpareInternalCommand {
    New { repo: String },
    Refresh { tree_id: Uuid },
}

const PROVISION_WAIT_SECS: u64 = 600;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = store::root_dir();
    let config_path = config::config_path();
    let result = migrate::run_if_needed(&root, &config_path)
        .and_then(|()| run(&root, &config_path, cli.command));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &Path, config_path: &Path, command: Command) -> Result<()> {
    match command {
        Command::Init {
            name,
            adopt,
            branch_prefix,
            redetect,
        } => repo::init(
            root,
            config_path,
            repo::InitOptions {
                name,
                adopt_path: adopt,
                branch_prefix,
                redetect,
            },
        ),
        Command::New {
            repo,
            name,
            branch,
            onto,
            profile,
            codex,
            claude,
            claudex,
            args,
        } => {
            let path = tree::new_tree(
                root,
                config_path,
                tree::NewOptions {
                    repo,
                    name,
                    branch,
                    onto,
                    profiles: profile,
                },
            )?;
            open_if_requested(
                root,
                &path,
                Agent::from_flags(codex, claude, claudex),
                &args,
            )
        }
        Command::Adopt {
            repo,
            name,
            branch,
            profile,
            codex,
            claude,
            claudex,
            args,
        } => {
            let path = tree::adopt(
                root,
                config_path,
                tree::AdoptOptions {
                    repo,
                    name,
                    branch,
                    profiles: profile,
                },
            )?;
            open_if_requested(
                root,
                &path,
                Agent::from_flags(codex, claude, claudex),
                &args,
            )
        }
        Command::Launch {
            worktree,
            repo,
            branch,
            onto,
            profile,
            claude,
            claudex,
            args,
        } => cmd_launch(
            root,
            config_path,
            LaunchArgs {
                worktree,
                repo,
                branch,
                onto,
                profile,
            },
            if claude {
                Agent::Claude
            } else if claudex {
                Agent::Claudex
            } else {
                Agent::Codex
            },
            &args,
        ),
        Command::Ls { repo, all, json } => cmd_ls(root, repo, all, json),
        Command::Path { selector } => cmd_path(root, &selector),
        Command::Name { path } => cmd_name(root, path),
        Command::Rm {
            selector,
            force,
            delete_branch,
            reparent_children,
        } => tree::rm_tree(
            root,
            config_path,
            &selector,
            force,
            delete_branch,
            reparent_children,
        ),
        Command::Status {
            selector,
            all,
            json,
        } => cmd_status(root, selector, all, json),
        Command::Wait { selector, timeout } => cmd_wait(root, selector, timeout),
        Command::Gc { repo, dry_run } => {
            tree::gc(root, config_path, tree::GcOptions { repo, dry_run })
        }
        Command::Doctor { fix } => tree::doctor(root, tree::DoctorOptions { fix }),
        Command::Sync { repo, stack } => sync::sync(root, config_path, repo, stack),
        Command::Restack { selector, dry_run } => cmd_restack(root, selector, dry_run),
        Command::Stack {
            selector,
            json,
            all,
            all_branches,
        } => cmd_stack(root, selector, json, all, all_branches),
        Command::Env { action } => match action {
            EnvCommand::Refresh { selector } => env_refresh::refresh(root, &selector),
        },
        Command::Spare { action, repo, json } => match action {
            None => cmd_spare_status(root, config_path, repo, json),
            Some(SpareCommand::Refresh { repo }) => cmd_spare_refresh(root, config_path, repo),
            Some(SpareCommand::Drop { repo }) => cmd_spare_drop(root, config_path, repo),
        },
        Command::Claude { target, args } => agent::exec_target(root, Agent::Claude, target, &args),
        Command::Codex { target, args } => agent::exec_target(root, Agent::Codex, target, &args),
        Command::Claudex { target, args } => {
            agent::exec_target(root, Agent::Claudex, target, &args)
        }
        Command::Provision { tree_id, profile } => {
            provision::run(root, config_path, tree_id, profile)
        }
        Command::SpareInternal { action } => match action {
            SpareInternalCommand::New { repo } => spare::provision_spare(root, config_path, &repo),
            SpareInternalCommand::Refresh { tree_id } => {
                spare::run_refresh(root, config_path, tree_id)
            }
        },
        Command::SessionContext { path } => context::session_context(root, path),
        Command::TabIndex => {
            println!("{}", tab::index()?);
            Ok(())
        }
        Command::LaunchPreview { selector } => cmd_launch_preview(root, config_path, &selector),
    }
}

/// Blocks until provisioning finishes before handing the tree over: a
/// session that opens mid-install hits a half-built tree, and nobody
/// remembers to wait by hand. A failed install refuses to open at all.
fn open_if_requested(
    root: &Path,
    tree_path: &Path,
    agent: Option<Agent>,
    args: &[String],
) -> Result<()> {
    let Some(agent) = agent else {
        return Ok(());
    };
    let id = store::load(root)?
        .trees
        .iter()
        .find(|t| t.path == tree_path)
        .map(|t| t.id)
        .with_context(|| format!("{} is not a registered tree", tree_path.display()))?;

    eprintln!("waiting for provisioning before opening a session...");
    wait_for_ready(root, id, PROVISION_WAIT_SECS).with_context(|| {
        format!(
            "not opening a session; the tree is still at {} — inspect it with `wt status`, then \
             `wt {}` into it once you know why",
            tree_path.display(),
            agent.executable()
        )
    })?;

    eprintln!(
        "provisioning finished; opening a {} session",
        agent.executable()
    );
    agent::exec_at(agent, tree_path, args, &[])
}

#[derive(Debug)]
enum LaunchPlan {
    /// `label` keeps the leading `@`, so a scratch session reads differently
    /// from a tree of the same name in the tab and the statusline.
    Scratch {
        repo: String,
        label: String,
    },
    Existing {
        id: Uuid,
    },
    New {
        repo: String,
        name: String,
    },
}

fn resolve_launch(
    store: &store::Store,
    worktree: &str,
    repo_arg: Option<&str>,
    has_branch_or_profile: bool,
    cwd_repo: Option<&str>,
) -> Result<LaunchPlan> {
    if let Some(label) = worktree.strip_prefix('@') {
        if label.is_empty() {
            bail!("a scratch session needs a name after '@', e.g. '@poking-around'");
        }
        if has_branch_or_profile {
            bail!(
                "a scratch session opens in the repo's base and creates nothing, so --branch, \
                 --onto, and --profile don't apply; drop them or drop the leading '@'"
            );
        }
        let repo = match repo_arg {
            Some(r) => r.to_string(),
            None => cwd_repo.map(str::to_string).with_context(|| {
                format!(
                    "'{worktree}' has no repo, and the current directory isn't inside a \
                     registered repo; pass one: wt launch {worktree} <repo>"
                )
            })?,
        };
        if !store.repos.contains_key(&repo) {
            bail!("unknown repo '{repo}'. Known repos: {}", known_repos(store));
        }
        return Ok(LaunchPlan::Scratch {
            repo,
            label: worktree.to_string(),
        });
    }

    if let Some(repo) = repo_arg {
        let existing = store.trees.iter().find(|t| {
            t.repo == repo
                && (t.name == worktree || tree::slugify(&t.name) == tree::slugify(worktree))
        });
        return Ok(match existing {
            Some(t) => LaunchPlan::Existing { id: t.id },
            None => LaunchPlan::New {
                repo: repo.to_string(),
                name: worktree.to_string(),
            },
        });
    }

    let matches: Vec<&store::Tree> = store
        .trees
        .iter()
        .filter(|t| t.name == worktree || tree::slugify(&t.name) == tree::slugify(worktree))
        .collect();

    match matches.len() {
        0 => bail!(
            "no tree named '{worktree}'; to create one, pass the repo: wt launch {worktree} <repo>"
        ),
        1 => Ok(LaunchPlan::Existing { id: matches[0].id }),
        _ => {
            if let Some(cwd_repo) = cwd_repo
                && let Some(t) = matches.iter().find(|t| t.repo == cwd_repo)
            {
                return Ok(LaunchPlan::Existing { id: t.id });
            }
            let candidates = matches
                .iter()
                .map(|t| format!("{}  {}", t.repo, t.name))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "'{worktree}' is ambiguous across repos: {candidates}; pass the repo: \
                 wt launch {worktree} <repo>"
            );
        }
    }
}

fn known_repos(store: &store::Store) -> String {
    if store.repos.is_empty() {
        "(none registered)".to_string()
    } else {
        store.repos.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// Returns the registered tree, whose name can differ from what the user
/// typed when the match came through the slug. Its name supplies Claude's
/// launch label and terminal color for either agent.
fn wait_for_tree(root: &Path, id: Uuid, agent: Agent) -> Result<store::Tree> {
    let pending_name = store::load(root)?
        .trees
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.name.clone())
        .unwrap_or_default();

    eprintln!("waiting for provisioning before opening a session...");
    wait_for_ready(root, id, PROVISION_WAIT_SECS).with_context(|| {
        format!(
            "not opening a session; inspect '{pending_name}' with `wt status`, then `wt {}` \
             into it once you know why",
            agent.executable()
        )
    })?;
    eprintln!(
        "provisioning finished; opening a {} session",
        agent.executable()
    );

    store::load(root)?
        .trees
        .into_iter()
        .find(|t| t.id == id)
        .with_context(|| format!("tree {id} is no longer registered"))
}

struct LaunchArgs {
    worktree: Option<String>,
    repo: Option<String>,
    branch: Option<String>,
    onto: Option<String>,
    profile: Option<Vec<String>>,
}

fn cmd_launch(
    root: &Path,
    config_path: &Path,
    launch: LaunchArgs,
    agent: Agent,
    args: &[String],
) -> Result<()> {
    let LaunchArgs {
        worktree,
        repo,
        branch,
        onto,
        profile,
    } = launch;
    let store = store::load(root)?;
    let config = config::load(config_path)?;
    let cwd = std::env::current_dir().context("reading current directory")?;
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let cwd_repo = store::repo_for_cwd(&store, &cwd);

    let plan = match worktree.as_deref() {
        Some(w) => resolve_launch(
            &store,
            w,
            repo.as_deref(),
            branch.is_some() || onto.is_some() || profile.is_some(),
            cwd_repo,
        )?,
        None => {
            if branch.is_some() || onto.is_some() || profile.is_some() {
                bail!(
                    "the picker only opens trees that already exist, so --branch, --onto, and \
                     --profile need a worktree name to create one: wt launch <worktree> [repo] \
                     --branch <b>"
                );
            }
            match pick::pick_tree(&store, cwd_repo)? {
                Some(id) => LaunchPlan::Existing { id },
                None => return Ok(()),
            }
        }
    };

    let (tree_path, color_repo, color_name, label) = match plan {
        LaunchPlan::Scratch { repo, label } => {
            let base = store
                .repos
                .get(&repo)
                .expect("resolve_launch already checked this repo is registered")
                .base
                .clone();
            eprintln!("opening a scratch session in {repo}'s base");
            let stripped = label.trim_start_matches('@').to_string();
            (base, repo, stripped, label)
        }
        LaunchPlan::Existing { id } => {
            let tree = wait_for_tree(root, id, agent)?;
            (tree.path, tree.repo, tree.name.clone(), tree.name)
        }
        LaunchPlan::New { repo, name } => {
            let path = tree::new_tree(
                root,
                config_path,
                tree::NewOptions {
                    repo: repo.clone(),
                    name,
                    branch,
                    onto,
                    profiles: profile,
                },
            )?;
            let id = store::load(root)?
                .trees
                .iter()
                .find(|t| t.path == path)
                .map(|t| t.id)
                .with_context(|| format!("{} is not a registered tree", path.display()))?;
            let tree = wait_for_tree(root, id, agent)?;
            (tree.path, tree.repo, tree.name.clone(), tree.name)
        }
    };

    let (color, hex) = color::pick(&color_repo, &color_name);

    let ctx = features::Context {
        tree_path: &tree_path,
        repo: &color_repo,
        label: &label,
        color_hex: hex,
    };
    let set_background_hook = config
        .features
        .terminal
        .as_ref()
        .and_then(|t| t.set_background.as_ref());

    let planter_enabled = config.features.planter.is_some();
    let planter_eligible = agent.planter_eligible(args);
    let mut env = Vec::new();
    let tab_index_str;

    if planter_enabled && planter_eligible {
        let get_position_hook = config
            .features
            .planter
            .as_ref()
            .and_then(|p| p.get_position.as_ref());
        let renumber_peers_hook = config
            .features
            .planter
            .as_ref()
            .and_then(|p| p.renumber_peers.as_ref());
        let tabs = features::get_position(get_position_hook, &ctx);
        if let Some(tabs) = &tabs {
            features::renumber_peers(renumber_peers_hook, tabs, &ctx);
        }

        env.push(("PLANTER_COLOR", color));
        env.push(("PLANTER_LABEL", &label));
        if let Some(idx) = tabs.as_ref().map(|t| t.mine) {
            tab_index_str = idx.to_string();
            env.push(("PLANTER_TAB_INDEX", &tab_index_str));
        }
    }

    features::set_background(set_background_hook, hex, &ctx);

    if agent.uses_claude_launch_contract() {
        return agent::exec_at(
            agent,
            &tree_path,
            &claude_launch_args(&label, args, color),
            &env,
        );
    }

    agent::exec_launch_codex(&tree_path, args, &env, planter_enabled && planter_eligible)
}

/// Claude takes the color as a slash-command prompt: the `--agent-color`
/// launch flag does not set the prompt-bar color.
fn claude_launch_args(name: &str, passthrough: &[String], color: &str) -> Vec<String> {
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

/// `gt create` moves a tree onto a new branch without updating the
/// registry, so `Tree.branch` — the branch a tree started on — drifts from
/// what's actually checked out. Falls back to the recorded branch on any
/// git failure (still provisioning, path gone) rather than showing nothing.
fn live_branch(t: &store::Tree) -> String {
    store::live_branch(t).unwrap_or_else(|| t.branch.clone())
}

/// `ls`'s STATE column for a spare: its own raw state never says "spare",
/// so a spare rendering next to ordinary trees under the same header still
/// reads as one.
fn ls_state_str(t: &store::Tree) -> String {
    if !t.spare {
        return state_str(t.state).to_string();
    }
    match t.state {
        store::TreeState::Ready => "spare".to_string(),
        store::TreeState::Provisioning => "spare:provisioning".to_string(),
        store::TreeState::Failed => "spare:failed".to_string(),
    }
}

fn cmd_ls(root: &Path, repo_filter: Option<String>, all: bool, json: bool) -> Result<()> {
    let store = store::load(root)?;
    let mut rows = Vec::new();
    for t in &store.trees {
        if !all && t.spare {
            continue;
        }
        if let Some(ref r) = repo_filter
            && &t.repo != r
        {
            continue;
        }
        let dirty = git::is_dirty(&t.path).unwrap_or(false);
        let branch = live_branch(t);
        rows.push((t, dirty, branch));
    }

    if json {
        let entries: Vec<_> = rows
            .iter()
            .map(|(t, dirty, branch)| {
                serde_json::json!({
                    "id": t.id,
                    "repo": t.repo,
                    "name": t.name,
                    "branch": branch,
                    "startingBranch": t.branch,
                    "path": t.path,
                    "created": t.created,
                    "state": t.state,
                    "dirty": dirty,
                    "spare": t.spare,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    let ids: Vec<String> = rows
        .iter()
        .map(|(t, _, _)| unique_id_prefix(&t.id, &store.trees))
        .collect();
    let states: Vec<String> = rows.iter().map(|(t, _, _)| ls_state_str(t)).collect();
    let w = |header: &str, vals: &mut dyn Iterator<Item = usize>| {
        vals.chain(std::iter::once(header.len())).max().unwrap_or(0)
    };
    let name_w = w(
        "NAME",
        &mut rows.iter().map(|(t, _, _)| t.name.chars().count()),
    );
    let repo_w = w(
        "REPO",
        &mut rows.iter().map(|(t, _, _)| t.repo.chars().count()),
    );
    let branch_w = w(
        "BRANCH",
        &mut rows.iter().map(|(_, _, branch)| branch.chars().count()),
    );
    let state_w = w("STATE", &mut states.iter().map(String::len));
    let id_w = w("UUID", &mut ids.iter().map(String::len));

    println!(
        "{:<name_w$} {:<repo_w$} {:<branch_w$} {:<state_w$} {:<id_w$} DIRTY",
        "NAME", "REPO", "BRANCH", "STATE", "UUID"
    );
    for (((t, dirty, branch), id), state) in rows.iter().zip(&ids).zip(&states) {
        println!(
            "{:<name_w$} {:<repo_w$} {:<branch_w$} {:<state_w$} {:<id_w$} {}",
            t.name,
            t.repo,
            branch,
            state,
            id,
            if *dirty { "dirty" } else { "" }
        );
    }
    Ok(())
}

fn cmd_restack(root: &Path, selector: Option<String>, dry_run: bool) -> Result<()> {
    let store = store::load(root)?;
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p));

    let (repo_name, current_branch) = stack_context(&store, selector.as_deref(), cwd.as_deref())?;
    let repo = store
        .repos
        .get(&repo_name)
        .with_context(|| format!("repo '{repo_name}' is not registered"))?;

    let Some(stacks) = stack::load(&repo_name, repo, &store)? else {
        return print_stack_unavailable(&repo_name, false);
    };

    let branch = current_branch.with_context(|| {
        "pass a worktree selector or a branch name, or run this from inside a registered tree or \
         repo"
    })?;
    if !stacks.graph.contains(&branch) {
        return print_branch_untracked(&repo_name, &branch, false);
    }

    let branches = stacks.graph.stack(&branch);
    let steps = restack::plan(&stacks, &branches, &store, repo);

    if steps.is_empty() {
        println!("nothing to restack");
        return Ok(());
    }

    if dry_run {
        print_restack_plan(&steps);
        return Ok(());
    }

    let offenders = restack::preflight(&steps);
    if !offenders.is_empty() {
        for o in &offenders {
            println!(
                "{} ({}): {}",
                o.label,
                o.dir.display(),
                o.reasons.join(", ")
            );
        }
        bail!(
            "refusing to restack: {} tree{} not ready",
            offenders.len(),
            if offenders.len() == 1 { " is" } else { "s are" }
        );
    }

    restack::execute(&steps)
}

fn print_restack_plan(steps: &[restack::Step]) {
    for step in steps {
        let parent = step
            .parent
            .as_deref()
            .map_or("none".to_string(), |p| format!("'{p}'"));
        println!(
            "would restack '{}' (parent {parent}) in {} ({})",
            step.branch,
            step.location.label(),
            step.dir.display(),
        );
    }
}

fn cmd_stack(
    root: &Path,
    selector: Option<String>,
    json: bool,
    all: bool,
    all_branches: bool,
) -> Result<()> {
    let store = store::load(root)?;
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p));

    let (repo_name, current_branch) = stack_context(&store, selector.as_deref(), cwd.as_deref())?;
    let repo = store
        .repos
        .get(&repo_name)
        .with_context(|| format!("repo '{repo_name}' is not registered"))?;

    let Some(stacks) = stack::load(&repo_name, repo, &store)? else {
        return print_stack_unavailable(&repo_name, json);
    };

    let branch_lists: Vec<Vec<String>> = if all {
        let mut roots = stacks.graph.roots();
        roots.sort();
        roots.iter().map(|r| stacks.graph.upstack(r)).collect()
    } else {
        let branch = current_branch.clone().with_context(|| {
            "pass a worktree selector or a branch name, or run this from inside a registered \
             tree or repo"
        })?;
        if !stacks.graph.contains(&branch) {
            return print_branch_untracked(&repo_name, &branch, json);
        }
        vec![stacks.graph.stack(&branch)]
    };

    if json {
        print_stack_json(
            &stacks,
            &branch_lists,
            current_branch.as_deref(),
            all_branches,
        )
    } else {
        print_stack_text(
            &stacks,
            &branch_lists,
            current_branch.as_deref(),
            all_branches,
        );
        Ok(())
    }
}

/// Resolves which repo `wt stack` looks at, and — for the default,
/// non-`--all` view — which branch counts as "current" for the `*` marker
/// and the stack it shows.
fn stack_context(
    store: &store::Store,
    selector: Option<&str>,
    cwd: Option<&Path>,
) -> Result<(String, Option<String>)> {
    if let Some(sel) = selector {
        if let Ok(t) = store::resolve(&store.trees, sel) {
            return Ok((t.repo.clone(), Some(live_branch(t))));
        }
        let repo = cwd
            .and_then(|c| store::repo_for_cwd(store, c))
            .map(str::to_string)
            .with_context(|| {
                format!(
                    "'{sel}' doesn't match a registered tree, and the current directory isn't \
                     inside a registered repo; run from inside one, or pass a tree selector \
                     instead of a branch name"
                )
            })?;
        return Ok((repo, Some(sel.to_string())));
    }

    let cwd = cwd.context("reading current directory")?;
    if let Some(tree) = store
        .trees
        .iter()
        .filter(|t| cwd.starts_with(&t.path))
        .max_by_key(|t| t.path.components().count())
    {
        return Ok((tree.repo.clone(), Some(live_branch(tree))));
    }
    if let Some((name, repo)) = store.repos.iter().find(|(_, r)| cwd.starts_with(&r.base)) {
        return Ok((name.clone(), git::current_branch(&repo.base).ok()));
    }
    bail!(
        "not inside a registered tree or repo; pass a selector, e.g. `wt stack <tree>` or \
         `wt stack --all`"
    );
}

fn print_stack_unavailable(repo_name: &str, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "available": false,
                "stacks": [],
            }))?
        );
    } else {
        println!(
            "no Graphite stack info for '{repo_name}': no readable .graphite_metadata.db in its \
             git dir (missing sqlite3, missing database, or an unexpected schema)"
        );
    }
    Ok(())
}

fn print_branch_untracked(repo_name: &str, branch: &str, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "available": true,
                "branchTracked": false,
                "stacks": [],
            }))?
        );
    } else {
        println!(
            "'{branch}' in '{repo_name}' isn't tracked by Graphite; run `gt track` if you want \
             it in a stack"
        );
    }
    Ok(())
}

fn holder_str(h: &stack::Holder) -> String {
    match h {
        stack::Holder::Tree { name, dirty, .. } => {
            if *dirty {
                format!("[{name}, dirty]")
            } else {
                format!("[{name}]")
            }
        }
        stack::Holder::Base => "[base]".to_string(),
        stack::Holder::Unregistered { path } => format!("[unregistered: {}]", path.display()),
        stack::Holder::None => String::new(),
    }
}

/// A short human label for a branch's pull request, e.g. `#14617 (ready to
/// merge)`. Empty when there's no PR to report.
fn pr_str(e: &stack::Entry) -> String {
    let Some(n) = e.pr_number else {
        return String::new();
    };
    let status = if e.pr_draft == Some(true) {
        "draft".to_string()
    } else {
        match (e.pr_state.as_deref(), e.pr_review_decision.as_deref()) {
            (Some("MERGED"), _) => "merged".to_string(),
            (Some("CLOSED"), _) => "closed".to_string(),
            (Some("OPEN"), Some("APPROVED")) => "ready to merge".to_string(),
            (Some("OPEN"), Some(decision)) => decision.to_lowercase().replace('_', " "),
            _ => "open".to_string(),
        }
    };
    format!(" #{n} ({status})")
}

fn print_stack_text(
    stacks: &stack::Stacks,
    branch_lists: &[Vec<String>],
    current: Option<&str>,
    all_branches: bool,
) {
    let mut hidden = 0usize;
    for (i, branches) in branch_lists.iter().enumerate() {
        if i > 0 {
            println!();
        }
        for entry in stacks.ordered(branches) {
            if !all_branches && entry.is_merged_or_closed() {
                hidden += 1;
                continue;
            }
            let depth = stacks.graph.downstack(&entry.branch).len();
            let marker = if Some(entry.branch.as_str()) == current {
                "*"
            } else {
                " "
            };
            let restack = match entry.needs_restack {
                Some(true) => " (needs restack)",
                _ => "",
            };
            let line = format!(
                "{marker} {}{}{restack}{}  {}",
                "  ".repeat(depth),
                entry.branch,
                pr_str(entry),
                holder_str(&entry.holder),
            );
            println!("{}", line.trim_end());
        }
    }
    if hidden > 0 {
        println!(
            "{hidden} merged or closed branch{} hidden; pass --all-branches to show them",
            if hidden == 1 { "" } else { "es" }
        );
    }
}

fn print_stack_json(
    stacks: &stack::Stacks,
    branch_lists: &[Vec<String>],
    current: Option<&str>,
    all_branches: bool,
) -> Result<()> {
    let mut hidden = 0usize;
    let mut stacks_json = Vec::with_capacity(branch_lists.len());
    for branches in branch_lists {
        let mut entries = Vec::new();
        for entry in stacks.ordered(branches) {
            if !all_branches && entry.is_merged_or_closed() {
                hidden += 1;
                continue;
            }
            entries.push(stack_entry_json(entry, current));
        }
        stacks_json.push(serde_json::json!({ "entries": entries }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "available": true,
            "stacks": stacks_json,
            "hiddenMergedOrClosed": hidden,
        }))?
    );
    Ok(())
}

fn stack_entry_json(e: &stack::Entry, current: Option<&str>) -> serde_json::Value {
    let holder = match &e.holder {
        stack::Holder::Tree { id, name, dirty } => serde_json::json!({
            "type": "tree",
            "id": id,
            "name": name,
            "dirty": dirty,
        }),
        stack::Holder::Base => serde_json::json!({ "type": "base" }),
        stack::Holder::Unregistered { path } => {
            serde_json::json!({ "type": "unregistered", "path": path })
        }
        stack::Holder::None => serde_json::json!({ "type": "none" }),
    };
    serde_json::json!({
        "branch": e.branch,
        "parent": e.parent,
        "needsRestack": e.needs_restack,
        "prNumber": e.pr_number,
        "prState": e.pr_state,
        "prReviewDecision": e.pr_review_decision,
        "prDraft": e.pr_draft,
        "current": current == Some(e.branch.as_str()),
        "holder": holder,
    })
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

pub(crate) fn status_state_str(t: &store::Tree) -> String {
    if proc::provisioning_is_stale(t) {
        format!("{} (stale)", state_str(t.state))
    } else {
        state_str(t.state).to_string()
    }
}

/// Each git-backed section degrades to `(unavailable)` on its own instead
/// of failing the whole preview: a tree still provisioning may have no
/// usable git dir yet.
fn cmd_launch_preview(root: &Path, config_path: &Path, selector: &str) -> Result<()> {
    let store = store::load(root)?;
    let t = store::resolve(&store.trees, selector)?;
    let config = config::load(config_path)?;

    println!("\x1b[1m{}\x1b[0m\x1b[2m  ·  {}\x1b[0m", t.name, t.repo);
    println!("\x1b[2m{}\x1b[0m", t.branch);
    println!("{}", collapse_home(&t.path));
    println!();

    let mut state_line = status_state_str(t);
    if t.state == store::TreeState::Provisioning {
        state_line.push_str(&format!("  {}", step_str(t)));
    }
    println!("state    {state_line}");
    let age = (Utc::now() - t.created).num_seconds();
    println!("created  {} ago", format_duration(age));

    let porcelain = git::status_porcelain(&t.path);
    match &porcelain {
        Ok(files) if files.is_empty() => println!("dirty    clean"),
        Ok(files) if files.len() == 1 => println!("dirty    1 file"),
        Ok(files) => println!("dirty    {} files", files.len()),
        Err(_) => println!("dirty    (unavailable)"),
    }
    println!();

    match config::repo(&config, &t.repo).ok().map(|r| r.trunk.clone()) {
        Some(trunk) => {
            println!("commits beyond origin/{trunk}");
            match git::log_oneline(&t.path, &format!("origin/{trunk}..HEAD"), 10) {
                Ok(lines) if lines.is_empty() => println!("  (none)"),
                Ok(lines) => {
                    for line in lines {
                        println!("  {line}");
                    }
                }
                Err(_) => println!("  (unavailable)"),
            }
        }
        None => {
            println!("commits beyond origin/trunk");
            println!("  (unavailable)");
        }
    }

    if let Ok(files) = &porcelain
        && !files.is_empty()
    {
        println!();
        for line in files.iter().take(12) {
            println!("  {line}");
        }
        if files.len() > 12 {
            println!("  … {} more", files.len() - 12);
        }
    }

    Ok(())
}

fn collapse_home(path: &Path) -> String {
    if let Ok(home) = std::env::var("HOME")
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

fn cmd_status(root: &Path, selector: Option<String>, all: bool, json: bool) -> Result<()> {
    let store = store::load(root)?;
    let trees: Vec<&store::Tree> = match &selector {
        Some(sel) => vec![store::resolve(&store.trees, sel)?],
        None => store
            .trees
            .iter()
            .filter(|t| t.state != store::TreeState::Ready)
            .filter(|t| all || !t.spare)
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
                    "stale": proc::provisioning_is_stale(t),
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
                    .filter(|t| t.state == store::TreeState::Provisioning && !t.spare)
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

fn cmd_spare_status(
    root: &Path,
    config_path: &Path,
    repo_filter: Option<String>,
    json: bool,
) -> Result<()> {
    let store = store::load(root)?;
    let config = config::load(config_path)?;
    let mut rows = Vec::new();
    for t in &store.trees {
        if !t.spare {
            continue;
        }
        if let Some(ref r) = repo_filter
            && &t.repo != r
        {
            continue;
        }
        let repo_config = config::repo(&config, &t.repo).ok();
        let head = git::rev_parse(&t.path, "HEAD").ok();
        let behind = match (repo_config, &head) {
            (Some(repo_config), Some(_)) => {
                git::rev_list_count(&t.path, &format!("HEAD..origin/{}", repo_config.trunk)).ok()
            }
            _ => None,
        };
        rows.push((t, head, behind));
    }

    if json {
        let entries: Vec<_> = rows
            .iter()
            .map(|(t, head, behind)| {
                serde_json::json!({
                    "repo": t.repo,
                    "id": t.id,
                    "state": t.state,
                    "stale": proc::provisioning_is_stale(t),
                    "head": head,
                    "createdSeconds": (Utc::now() - t.created).num_seconds().max(0),
                    "behindTrunk": behind,
                    "logPath": t.log_path,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("no hot spares");
        return Ok(());
    }

    println!(
        "{:<12} {:<21} {:<10} {:<8} {:<8} LOG",
        "REPO", "STATE", "HEAD", "AGE", "BEHIND"
    );
    for (t, head, behind) in &rows {
        let age = format_duration((Utc::now() - t.created).num_seconds());
        let head = head.as_deref().map_or("-", |h| &h[..h.len().min(8)]);
        let behind = behind.map_or("-".to_string(), |n| n.to_string());
        let log = t
            .log_path
            .as_ref()
            .map_or("-".to_string(), |p| p.display().to_string());
        println!(
            "{:<12} {:<21} {:<10} {:<8} {:<8} {}",
            t.repo,
            status_state_str(t),
            head,
            age,
            behind,
            log
        );
    }
    Ok(())
}

fn cmd_spare_refresh(root: &Path, config_path: &Path, repo: Option<String>) -> Result<()> {
    spare::refresh(root, config_path, repo.as_deref())?;
    println!("refreshing");
    Ok(())
}

/// Resolves `--repo`, else the current directory's repo, and errors
/// naming the registered repos rather than falling back to "every repo" —
/// a bare `wt spare drop` must never silently turn spares off everywhere.
fn cmd_spare_drop(root: &Path, config_path: &Path, repo: Option<String>) -> Result<()> {
    let repo_name = match repo {
        Some(r) => r,
        None => {
            let store = store::load(root)?;
            let cwd = std::env::current_dir().context("reading current directory")?;
            let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
            store::repo_for_cwd(&store, &cwd)
                .map(str::to_string)
                .with_context(|| {
                    format!(
                        "pass --repo; the current directory isn't inside a registered repo. \
                         Known repos: {}",
                        known_repos(&store)
                    )
                })?
        }
    };
    spare::drop_spare(root, config_path, &repo_name)?;
    println!("dropped {repo_name}'s hot spare; spares are now off for it");
    Ok(())
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
            parent_branch: None,
            spare: false,
        }
    }

    #[test]
    fn claude_launch_args_orders_name_flag_passthrough_then_color_last() {
        let args = claude_launch_args(
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

    fn sample_repo(base: &str) -> store::Repo {
        store::Repo {
            base: PathBuf::from(base),
            last_fetch: None,
        }
    }

    fn store_with(repos: &[(&str, &str)], trees: Vec<Tree>) -> store::Store {
        let mut repos_map = std::collections::BTreeMap::new();
        for (name, base) in repos {
            repos_map.insert((*name).to_string(), sample_repo(base));
        }
        store::Store {
            repos: repos_map,
            trees,
            ..Default::default()
        }
    }

    fn tree_in(id: &str, repo: &str, name: &str) -> Tree {
        Tree {
            id: id.parse().unwrap(),
            repo: repo.into(),
            name: name.into(),
            branch: format!("josh/{}", tree::slugify(name)),
            path: PathBuf::from(format!("/tmp/{name}")),
            created: Utc::now(),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
            parent_branch: None,
            spare: false,
        }
    }

    #[test]
    fn resolve_launch_repo_less_existing_tree_finds_its_repo() {
        let t = tree_in(
            "019fa4ef-6669-7f32-a29c-a459aee6716b",
            "monorepo",
            "fix login",
        );
        let store = store_with(&[("monorepo", "/base")], vec![t.clone()]);

        match resolve_launch(&store, "fix login", None, false, None).unwrap() {
            LaunchPlan::Existing { id } => assert_eq!(id, t.id),
            _ => panic!("expected an existing tree"),
        }
    }

    #[test]
    fn resolve_launch_no_match_without_repo_errors_and_creates_nothing() {
        let store = store_with(&[("monorepo", "/base")], vec![]);
        let err = resolve_launch(&store, "ghost", None, false, None).unwrap_err();
        assert!(
            err.to_string().contains("no tree named 'ghost'"),
            "message was: {err}"
        );
    }

    #[test]
    fn resolve_launch_ambiguous_name_across_repos_names_both_candidates() {
        let a = tree_in(
            "019fa4ef-6669-7f32-a29c-a459aee6716b",
            "repo-a",
            "shared name",
        );
        let b = tree_in(
            "019fa4ef-e6e2-78c2-977a-f55f5f00ab25",
            "repo-b",
            "shared name",
        );
        let store = store_with(&[("repo-a", "/a"), ("repo-b", "/b")], vec![a, b]);

        let err = resolve_launch(&store, "shared name", None, false, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("repo-a  shared name"), "message was: {msg}");
        assert!(msg.contains("repo-b  shared name"), "message was: {msg}");
    }

    #[test]
    fn resolve_launch_ambiguous_name_is_broken_by_the_cwd_repo() {
        let a = tree_in(
            "019fa4ef-6669-7f32-a29c-a459aee6716b",
            "repo-a",
            "shared name",
        );
        let b = tree_in(
            "019fa4ef-e6e2-78c2-977a-f55f5f00ab25",
            "repo-b",
            "shared name",
        );
        let store = store_with(&[("repo-a", "/a"), ("repo-b", "/b")], vec![a.clone(), b]);

        match resolve_launch(&store, "shared name", None, false, Some("repo-a")).unwrap() {
            LaunchPlan::Existing { id } => assert_eq!(id, a.id),
            _ => panic!("expected the cwd repo's tree"),
        }
    }

    #[test]
    fn resolve_launch_plain_name_with_repo_creates_when_no_tree_matches() {
        let store = store_with(&[("monorepo", "/base")], vec![]);

        match resolve_launch(&store, "fix login", Some("monorepo"), false, None).unwrap() {
            LaunchPlan::New { repo, name } => {
                assert_eq!(repo, "monorepo");
                assert_eq!(name, "fix login");
            }
            _ => panic!("expected a new-tree plan"),
        }
    }

    #[test]
    fn resolve_launch_scratch_with_unknown_repo_errors() {
        let store = store_with(&[], vec![]);
        let err = resolve_launch(&store, "@poking-around", Some("bogus"), false, None).unwrap_err();
        assert!(
            err.to_string().contains("unknown repo"),
            "message was: {err}"
        );
    }

    #[test]
    fn resolve_launch_scratch_with_branch_or_profile_errors() {
        let store = store_with(&[("monorepo", "/base")], vec![]);
        let err =
            resolve_launch(&store, "@poking-around", Some("monorepo"), true, None).unwrap_err();
        assert!(err.to_string().contains("--branch"), "message was: {err}");
    }

    #[test]
    fn resolve_launch_scratch_infers_repo_from_cwd() {
        let store = store_with(&[("monorepo", "/base")], vec![]);

        match resolve_launch(&store, "@poking-around", None, false, Some("monorepo")).unwrap() {
            LaunchPlan::Scratch { repo, label } => {
                assert_eq!(repo, "monorepo");
                assert_eq!(label, "@poking-around");
            }
            _ => panic!("expected a scratch session"),
        }
    }

    #[test]
    fn resolve_launch_scratch_without_repo_or_cwd_errors() {
        let store = store_with(&[("monorepo", "/base")], vec![]);
        let err = resolve_launch(&store, "@poking-around", None, false, None).unwrap_err();
        assert!(err.to_string().contains("repo"), "message was: {err}");
    }
}
