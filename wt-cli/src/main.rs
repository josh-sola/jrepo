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
use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand};
use uuid::Uuid;

use agent::Agent;

#[derive(Parser)]
#[command(
    name = "wt",
    about = "Manage adopted repositories and ready-to-use Git worktrees",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage registered base repositories.
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
    /// Create, inspect, and remove worktrees.
    Tree {
        #[command(subcommand)]
        command: TreeCommand,
    },
    /// Work with Graphite stacks across worktrees.
    Gt {
        #[command(subcommand)]
        command: GtCommand,
    },
    /// Maintain the registry and remove unused trees.
    Upkeep {
        #[command(subcommand)]
        command: UpkeepCommand,
    },
    /// Run a coding agent in a repository or tree.
    Llm {
        #[command(subcommand)]
        command: LlmCommand,
    },
    /// Open a session in a tree, creating it when requested.
    #[command(
        long_about = "Open Pi in an existing tree or create one when no match is found with --repo. Select another agent with --claude or --codex.\n\nWith no TREE, open the existing tree picker. A TREE starting with @ opens a labeled scratch session in a repository base and creates nothing; use --repo or run from that repository.\n\nExamples:\n  wt go fix-login --repo monorepo\n  wt go @poking-around --repo monorepo\n  wt go --codex -- --model gpt-5"
    )]
    Go(GoArgs),
    /// Change directory to a tree through installed shell integration.
    Cd {
        #[arg(
            value_name = "TREE",
            help = "Tree name, UUID, UUID prefix, unique name substring, or branch name."
        )]
        tree: String,
    },
    /// Show command help or the recursive command tree.
    Help(HelpArgs),
    /// Runs a tree's provisioning steps; spawned detached by `wt tree new`.
    #[command(name = "__provision", hide = true)]
    Provision {
        tree_id: Uuid,
        #[arg(long, value_delimiter = ',')]
        profile: Option<Vec<String>>,
    },
    /// Builds or refreshes one hot spare; spawned detached by `wt tree new`,
    /// `wt repo sync`, and `wt repo spare refresh`.
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
    /// Prints this tab's 1-based rank among planted agent tabs in the
    /// caller's own Ghostty window.
    #[command(name = "__tab-index", hide = true)]
    TabIndex,
    /// Prints a tree's details for the `wt go` fzf preview window.
    #[command(name = "__launch-preview", hide = true)]
    LaunchPreview { selector: String },
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum RepoCommand {
    /// Register an existing clone as a repository's base.
    Adopt {
        #[arg(
            value_name = "REPO",
            help = "Short name to register for this repository."
        )]
        name: String,
        #[arg(value_name = "PATH", help = "Existing Git clone to use as the base.")]
        adopt: PathBuf,
        #[arg(
            long,
            value_name = "PREFIX",
            help = "Branch prefix for a new repository (default: josh/). Existing config keeps its value."
        )]
        branch_prefix: Option<String>,
        #[arg(
            long,
            help = "Replace only detected provisioning steps in an existing config block."
        )]
        redetect: bool,
    },
    /// Fetch and fast-forward a base's trunk.
    Sync {
        #[arg(
            value_name = "REPO",
            help = "Repository to sync. Omit to sync every registered repository."
        )]
        repo: Option<String>,
        #[arg(
            long,
            help = "Also restack every cross-tree Graphite stack after syncing."
        )]
        stack: bool,
    },
    /// Move uncommitted base work into a fresh tree.
    #[command(
        long_about = "Move tracked and untracked uncommitted changes out of a repository base into a fresh tree. A clean base is refused. If applying the preserved stash conflicts, the stash is kept."
    )]
    Lift(LiftArgs),
    /// Show or manage a repository's hot spare.
    Spare(SpareArgs),
}

#[derive(Args)]
#[command(group(ArgGroup::new("agent").args(["pi", "codex", "claude"])))]
struct LiftArgs {
    #[arg(long, help = "Open Pi after provisioning.")]
    pi: bool,
    #[arg(
        value_name = "REPO",
        help = "Repository whose base to lift. Omit only from inside that base."
    )]
    repo: Option<String>,
    #[arg(
        long,
        value_name = "SUMMARY",
        help = "Human-readable name for the new tree."
    )]
    name: String,
    #[arg(
        long,
        value_name = "BRANCH",
        help = "Exact branch name for the new tree."
    )]
    branch: Option<String>,
    #[arg(
        long,
        value_name = "PROFILE,...",
        value_delimiter = ',',
        help = "Comma-separated provisioning profiles. Omit to run every configured profile."
    )]
    profile: Option<Vec<String>>,
    #[arg(
        long,
        conflicts_with = "claude",
        help = "Open Codex after provisioning."
    )]
    codex: bool,
    #[arg(
        long,
        conflicts_with = "codex",
        help = "Open Claude after provisioning."
    )]
    claude: bool,
    #[arg(
        last = true,
        requires = "agent",
        value_name = "AGENT_ARGS",
        help = "Arguments passed through to the selected agent after --."
    )]
    args: Vec<String>,
}

#[derive(Args)]
struct SpareArgs {
    #[command(subcommand)]
    action: Option<SpareCommand>,
    #[arg(
        long,
        value_name = "REPO",
        help = "Repository to show. Omit to show all repositories."
    )]
    repo: Option<String>,
    #[arg(long, help = "Write JSON instead of a table.")]
    json: bool,
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum TreeCommand {
    /// Create and provision a worktree.
    New(NewArgs),
    /// List registered worktrees.
    Ls(ListArgs),
    /// Print a worktree's absolute path.
    Path {
        #[arg(
            value_name = "TREE",
            help = "Tree name, UUID, UUID prefix, unique name substring, or branch name."
        )]
        selector: String,
    },
    /// Print the worktree name containing a path.
    Name {
        #[arg(
            long,
            value_name = "PATH",
            help = "Path to inspect. Defaults to the current directory."
        )]
        path: Option<PathBuf>,
    },
    /// Remove a worktree.
    Rm {
        #[arg(
            value_name = "TREE",
            help = "Tree name, UUID, UUID prefix, unique name substring, or branch name."
        )]
        selector: String,
        #[arg(
            long,
            help = "Bypass dirty, unpushed-commit, and Graphite-child safety checks."
        )]
        force: bool,
        #[arg(
            long,
            help = "Also delete the tree's branch when its commits are preserved elsewhere."
        )]
        delete_branch: bool,
        #[arg(
            long,
            requires = "delete_branch",
            help = "Re-parent Graphite children before deleting the branch."
        )]
        reparent_children: bool,
    },
    /// Show provisioning status.
    Status {
        #[arg(
            value_name = "TREE",
            help = "Tree name, UUID, UUID prefix, unique name substring, or branch name. Omit to show non-ready ordinary trees."
        )]
        selector: Option<String>,
        #[arg(long, help = "Also include non-ready hot spares.")]
        all: bool,
        #[arg(long, help = "Write JSON instead of a table.")]
        json: bool,
    },
    /// Wait for provisioning to finish.
    Wait {
        #[arg(
            value_name = "TREE",
            help = "Tree name, UUID, UUID prefix, unique name substring, or branch name. Omit only when exactly one ordinary tree is provisioning."
        )]
        selector: Option<String>,
        #[arg(
            long,
            default_value_t = 600,
            value_name = "SECONDS",
            help = "Maximum wait time in seconds (default: 600)."
        )]
        timeout: u64,
    },
    /// Copy configured env files from a base into a tree.
    Env {
        #[arg(
            value_name = "TREE",
            help = "Tree name, UUID, UUID prefix, unique name substring, or branch name. Matching env files are overwritten with the base's current copies; this does not regenerate values."
        )]
        selector: String,
    },
}

#[derive(Args)]
#[command(group(ArgGroup::new("agent").args(["pi", "codex", "claude"])))]
struct NewArgs {
    #[arg(long, help = "Open Pi after provisioning.")]
    pi: bool,
    #[arg(
        value_name = "REPO",
        help = "Registered repository in which to create the tree."
    )]
    repo: String,
    #[arg(
        long,
        value_name = "SUMMARY",
        help = "Human-readable tree name; also supplies the generated branch suffix."
    )]
    name: String,
    #[arg(
        long,
        value_name = "BRANCH",
        help = "Exact branch to create. Defaults to the configured prefix plus a slug of --name."
    )]
    branch: Option<String>,
    #[arg(
        long,
        value_name = "TREE_OR_REF",
        help = "Tree branch, local branch, or commit to branch from instead of trunk."
    )]
    onto: Option<String>,
    #[arg(
        long,
        value_name = "PROFILE,...",
        value_delimiter = ',',
        help = "Comma-separated provisioning profiles. Omit to run every configured profile."
    )]
    profile: Option<Vec<String>>,
    #[arg(
        long,
        conflicts_with = "claude",
        help = "Open Codex after provisioning."
    )]
    codex: bool,
    #[arg(
        long,
        conflicts_with = "codex",
        help = "Open Claude after provisioning."
    )]
    claude: bool,
    #[arg(
        last = true,
        requires = "agent",
        value_name = "AGENT_ARGS",
        help = "Arguments passed through to the selected agent after --."
    )]
    args: Vec<String>,
}

#[derive(Args)]
struct ListArgs {
    #[arg(
        long,
        value_name = "REPO",
        help = "Only list trees for this registered repository."
    )]
    repo: Option<String>,
    #[arg(long, help = "Also show each repository's hot spare.")]
    all: bool,
    #[arg(long, help = "Write JSON instead of a table.")]
    json: bool,
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum GtCommand {
    /// Show the stack containing a tree or branch.
    Stack {
        #[arg(
            value_name = "TREE_OR_BRANCH",
            help = "Tree selector or branch. A branch needs the current directory to identify its repository."
        )]
        selector: Option<String>,
        #[arg(long, help = "Write JSON instead of text.")]
        json: bool,
        #[arg(long, help = "Show every stack in the current repository.")]
        all: bool,
        #[arg(long, help = "Include merged and closed branches.")]
        all_branches: bool,
    },
    /// Restack a Graphite stack across its worktrees.
    Restack {
        #[arg(
            value_name = "TREE_OR_BRANCH",
            help = "Tree selector or branch. A branch needs the current directory to identify its repository."
        )]
        selector: Option<String>,
        #[arg(long, help = "Print the ordered plan without changing anything.")]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum UpkeepCommand {
    /// Reap safe, unused trees.
    Gc {
        #[arg(
            long,
            value_name = "REPO",
            help = "Only reap trees for this registered repository."
        )]
        repo: Option<String>,
        #[arg(long, help = "Report candidates without changing anything.")]
        dry_run: bool,
    },
    /// Reconcile the registry with Git's worktree list.
    Doctor {
        #[arg(
            long,
            help = "Remove stale registry entries and prune Git worktree metadata."
        )]
        fix: bool,
    },
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum LlmCommand {
    /// Run Pi with its working directory set.
    Pi(AgentArgs),
    /// Run Claude with its working directory set.
    Claude(AgentArgs),
    /// Run Codex with its working directory set.
    Codex(AgentArgs),
}

#[derive(Args)]
struct AgentArgs {
    #[arg(
        value_name = "TREE_OR_REPO",
        help = "Registered repository base or tree selector. Omit to use the current registered tree or base."
    )]
    target: Option<String>,
    #[arg(
        last = true,
        value_name = "AGENT_ARGS",
        help = "Arguments passed through unchanged after --. Targeting a base prints a warning."
    )]
    args: Vec<String>,
}

#[derive(Args)]
#[command(group(ArgGroup::new("agent").args(["pi", "codex", "claude"])))]
struct GoArgs {
    #[arg(
        value_name = "TREE",
        help = "Existing tree name, UUID, UUID prefix, unique name substring, or branch; creates a new tree name when --repo is supplied. Omit for the picker."
    )]
    worktree: Option<String>,
    #[arg(
        long,
        value_name = "REPO",
        help = "Scope tree matching and create a tree here when no tree matches."
    )]
    repo: Option<String>,
    #[arg(
        long,
        value_name = "BRANCH",
        help = "Exact branch for a newly created tree."
    )]
    branch: Option<String>,
    #[arg(
        long,
        value_name = "TREE_OR_REF",
        help = "Tree branch, local branch, or commit to branch from when creating."
    )]
    onto: Option<String>,
    #[arg(
        long,
        value_name = "PROFILE,...",
        value_delimiter = ',',
        help = "Comma-separated provisioning profiles for a newly created tree."
    )]
    profile: Option<Vec<String>>,
    #[arg(long, help = "Open Pi (default).")]
    pi: bool,
    #[arg(long, help = "Open Codex instead of Pi.")]
    codex: bool,
    #[arg(long, help = "Open Claude instead of Pi.")]
    claude: bool,
    #[arg(
        last = true,
        value_name = "AGENT_ARGS",
        help = "Arguments passed through unchanged to the selected agent after --."
    )]
    args: Vec<String>,
}

#[derive(Args)]
struct HelpArgs {
    #[arg(short, long, help = "Show the complete public command hierarchy.")]
    recursive: bool,
    #[arg(value_name = "COMMAND", num_args = 0.., help = "Optional command path to describe.")]
    path: Vec<String>,
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum SpareCommand {
    /// Force a refresh now instead of waiting for `wt repo sync`.
    Refresh {
        #[arg(
            long,
            value_name = "REPO",
            help = "Repository to refresh. Omit to refresh all repositories."
        )]
        repo: Option<String>,
    },
    /// Remove a repo's spare and turn spares off so later syncs do not rebuild it.
    Drop {
        #[arg(
            long,
            value_name = "REPO",
            help = "Repository whose spare to drop. Omit to use the current repository."
        )]
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
    let cli = Cli::parse_from(normalize_legacy_args(std::env::args_os()));
    let root = store::root_dir();
    let config_path = config::config_path();
    let result = if matches!(&cli.command, Command::Help(_)) {
        run(&root, &config_path, cli.command)
    } else {
        migrate::run_if_needed(&root, &config_path)
            .and_then(|()| run(&root, &config_path, cli.command))
    };
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
        Command::Repo { command } => match command {
            RepoCommand::Adopt {
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
            RepoCommand::Sync { repo, stack } => cmd_sync(root, config_path, repo, stack),
            RepoCommand::Lift(args) => {
                reject_unselected_agent_args(args.pi, args.codex, args.claude, &args.args)?;
                let path = tree::adopt(
                    root,
                    config_path,
                    tree::AdoptOptions {
                        repo: args.repo,
                        name: args.name,
                        branch: args.branch,
                        profiles: args.profile,
                    },
                )?;
                open_if_requested(
                    root,
                    &path,
                    Agent::from_flags(args.pi, args.codex, args.claude),
                    &args.args,
                )
            }
            RepoCommand::Spare(args) => match args.action {
                None => cmd_spare_status(root, config_path, args.repo, args.json),
                Some(action) => {
                    if args.repo.is_some() || args.json {
                        bail!(
                            "place --repo after the spare action; for example: wt repo spare refresh --repo <REPO>"
                        );
                    }
                    match action {
                        SpareCommand::Refresh { repo } => {
                            cmd_spare_refresh(root, config_path, repo)
                        }
                        SpareCommand::Drop { repo } => cmd_spare_drop(root, config_path, repo),
                    }
                }
            },
        },
        Command::Tree { command } => match command {
            TreeCommand::New(args) => {
                reject_unselected_agent_args(args.pi, args.codex, args.claude, &args.args)?;
                let path = tree::new_tree(
                    root,
                    config_path,
                    tree::NewOptions {
                        repo: args.repo,
                        name: args.name,
                        branch: args.branch,
                        onto: args.onto,
                        profiles: args.profile,
                    },
                )?;
                open_if_requested(
                    root,
                    &path,
                    Agent::from_flags(args.pi, args.codex, args.claude),
                    &args.args,
                )
            }
            TreeCommand::Ls(args) => cmd_ls(root, args.repo, args.all, args.json),
            TreeCommand::Path { selector } => cmd_path(root, &selector),
            TreeCommand::Name { path } => cmd_name(root, path),
            TreeCommand::Rm {
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
            TreeCommand::Status {
                selector,
                all,
                json,
            } => cmd_status(root, selector, all, json),
            TreeCommand::Wait { selector, timeout } => cmd_wait(root, selector, timeout),
            TreeCommand::Env { selector } => env_refresh::refresh(root, &selector),
        },
        Command::Gt { command } => match command {
            GtCommand::Stack {
                selector,
                json,
                all,
                all_branches,
            } => cmd_stack(root, selector, json, all, all_branches),
            GtCommand::Restack { selector, dry_run } => cmd_restack(root, selector, dry_run),
        },
        Command::Upkeep { command } => match command {
            UpkeepCommand::Gc { repo, dry_run } => {
                validate_repo_filter(root, repo.as_deref())?;
                tree::gc(root, config_path, tree::GcOptions { repo, dry_run })
            }
            UpkeepCommand::Doctor { fix } => tree::doctor(root, tree::DoctorOptions { fix }),
        },
        Command::Llm { command } => match command {
            LlmCommand::Pi(args) => agent::exec_target(root, Agent::Pi, args.target, &args.args),
            LlmCommand::Claude(args) => {
                agent::exec_target(root, Agent::Claude, args.target, &args.args)
            }
            LlmCommand::Codex(args) => {
                agent::exec_target(root, Agent::Codex, args.target, &args.args)
            }
        },
        Command::Go(args) => cmd_launch(
            root,
            config_path,
            LaunchArgs {
                worktree: args.worktree,
                repo: args.repo,
                branch: args.branch,
                onto: args.onto,
                profile: args.profile,
            },
            Agent::from_flags(args.pi, args.codex, args.claude).unwrap_or(Agent::Pi),
            &args.args,
        ),
        Command::Cd { tree } => bail!(
            "`wt cd` needs the installed shell integration because a program cannot change its parent shell's directory; run `wt tree path {tree}` or reinstall wt and open a new shell"
        ),
        Command::Help(args) => cmd_help(args),
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

fn reject_unselected_agent_args(
    pi: bool,
    codex: bool,
    claude: bool,
    args: &[String],
) -> Result<()> {
    if !args.is_empty() && !pi && !codex && !claude {
        bail!("arguments after -- require --pi, --codex, or --claude");
    }
    Ok(())
}

fn cmd_sync(root: &Path, config_path: &Path, repo: Option<String>, stack: bool) -> Result<()> {
    validate_repo_filter(root, repo.as_deref())?;
    sync::sync(root, config_path, repo, stack)
}

fn validate_repo_filter(root: &Path, repo: Option<&str>) -> Result<()> {
    let Some(repo) = repo else {
        return Ok(());
    };
    let store = store::load(root)?;
    if !store.repos.contains_key(repo) {
        bail!(
            "unknown repo '{repo}'. Known repos: {}",
            known_repos(&store)
        );
    }
    Ok(())
}

fn cmd_help(args: HelpArgs) -> Result<()> {
    let command = public_command_for_path(&args.path)?;
    let command_path = args.path.iter().map(String::as_str).collect::<Vec<_>>();
    if args.recursive {
        print!("{}", recursive_help(&command, &command_path));
        return Ok(());
    }
    print!("{}", detailed_help(command, &command_path));
    Ok(())
}

fn public_command_for_path(path: &[String]) -> Result<clap::Command> {
    let mut command = Cli::command();
    for component in path {
        let children = visible_children(&command)
            .map(|child| child.get_name().to_string())
            .collect::<Vec<_>>();
        let next = visible_children(&command)
            .find(|child| child.get_name() == component)
            .cloned()
            .with_context(|| {
                format!(
                    "unknown command '{component}'. Valid commands: {}",
                    if children.is_empty() {
                        "(none)".to_string()
                    } else {
                        children.join(", ")
                    }
                )
            })?;
        command = next;
    }
    Ok(command)
}

fn detailed_help(mut command: clap::Command, path: &[&str]) -> String {
    let invocation = if path.is_empty() {
        "wt".to_string()
    } else {
        format!("wt {}", path.join(" "))
    };
    command.set_bin_name(invocation);
    command.render_long_help().to_string()
}

fn visible_children(command: &clap::Command) -> impl Iterator<Item = &clap::Command> {
    command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
}

fn recursive_help(command: &clap::Command, path: &[&str]) -> String {
    let display = if path.is_empty() {
        "wt".to_string()
    } else {
        format!("wt {}", path.join(" "))
    };
    let mut out = display;
    if let Some(about) = command.get_about() {
        out.push_str(&format!(" -- {about}"));
    }
    out.push('\n');
    let children = visible_children(command).collect::<Vec<_>>();
    for (index, child) in children.iter().enumerate() {
        render_command_tree(child, "", index + 1 == children.len(), &mut out);
    }
    out
}

fn render_command_tree(command: &clap::Command, prefix: &str, last: bool, out: &mut String) {
    out.push_str(prefix);
    out.push_str(if last { "└── " } else { "├── " });
    out.push_str(command.get_name());
    if let Some(about) = command.get_about() {
        out.push_str(&format!(" -- {about}"));
    }
    out.push('\n');
    let next_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
    let children = visible_children(command).collect::<Vec<_>>();
    for (index, child) in children.iter().enumerate() {
        render_command_tree(child, &next_prefix, index + 1 == children.len(), out);
    }
}

fn normalize_legacy_args(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    let mut args = args.into_iter();
    let Some(program) = args.next() else {
        return Vec::new();
    };
    let rest = args.collect::<Vec<_>>();
    let words = rest
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    let canonical = match words.first().map(|word| word.as_ref()) {
        Some("new") => prepend_words(&rest, &["tree", "new"]),
        Some("ls") => prepend_words(&rest, &["tree", "ls"]),
        Some("path") => prepend_words(&rest, &["tree", "path"]),
        Some("name") => prepend_words(&rest, &["tree", "name"]),
        Some("rm") => prepend_words(&rest, &["tree", "rm"]),
        Some("status") => prepend_words(&rest, &["tree", "status"]),
        Some("wait") => prepend_words(&rest, &["tree", "wait"]),
        Some("stack") => prepend_words(&rest, &["gt", "stack"]),
        Some("restack") => prepend_words(&rest, &["gt", "restack"]),
        Some("gc") => prepend_words(&rest, &["upkeep", "gc"]),
        Some("doctor") => prepend_words(&rest, &["upkeep", "doctor"]),
        Some("sync") => prepend_words(&rest, &["repo", "sync"]),
        Some("spare") => prepend_words(&rest, &["repo", "spare"]),
        Some("claude") => prepend_words(&rest, &["llm", "claude"]),
        Some("codex") => prepend_words(&rest, &["llm", "codex"]),
        Some("adopt") => prepend_words(&rest, &["repo", "lift"]),
        Some("env") if words.get(1).is_some_and(|word| word == "refresh") => {
            let mut rewritten = vec![
                std::ffi::OsString::from("tree"),
                std::ffi::OsString::from("env"),
            ];
            rewritten.extend(rest.into_iter().skip(2));
            rewritten
        }
        Some("init") => normalize_legacy_init(&rest),
        Some("launch") => normalize_legacy_launch(&rest),
        _ => rest,
    };
    std::iter::once(program).chain(canonical).collect()
}

fn prepend_words(rest: &[std::ffi::OsString], prefix: &[&str]) -> Vec<std::ffi::OsString> {
    prefix
        .iter()
        .map(std::ffi::OsString::from)
        .chain(rest.iter().skip(1).cloned())
        .collect()
}

fn normalize_legacy_init(rest: &[std::ffi::OsString]) -> Vec<std::ffi::OsString> {
    let mut out = vec![
        std::ffi::OsString::from("repo"),
        std::ffi::OsString::from("adopt"),
    ];
    let mut iter = rest.iter().skip(1);
    if let Some(name) = iter.next() {
        out.push(name.clone());
    }
    let mut path = None;
    while let Some(arg) = iter.next() {
        if arg == "--adopt" {
            path = iter.next().cloned();
        } else if let Some(value) = arg.to_string_lossy().strip_prefix("--adopt=") {
            path = Some(std::ffi::OsString::from(value));
        } else {
            out.push(arg.clone());
        }
    }
    if let Some(path) = path {
        out.insert(3, path);
    }
    out
}

fn normalize_legacy_launch(rest: &[std::ffi::OsString]) -> Vec<std::ffi::OsString> {
    let mut remainder = Vec::new();
    let mut positionals = Vec::new();
    let mut after_separator = false;
    let mut option_value_follows = false;
    for arg in rest.iter().skip(1) {
        let arg_text = arg.to_string_lossy();
        if after_separator {
            remainder.push(arg.clone());
        } else if option_value_follows {
            remainder.push(arg.clone());
            option_value_follows = false;
        } else if arg == "--" {
            remainder.push(arg.clone());
            after_separator = true;
        } else if matches!(arg_text.as_ref(), "--branch" | "--onto" | "--profile") {
            remainder.push(arg.clone());
            option_value_follows = true;
        } else if arg_text.starts_with('-') {
            remainder.push(arg.clone());
        } else if positionals.len() < 2 {
            positionals.push(arg.clone());
        } else {
            remainder.push(arg.clone());
        }
    }
    let mut out = vec![std::ffi::OsString::from("go")];
    if let Some(tree) = positionals.first() {
        out.push(tree.clone());
    }
    if let Some(repo) = positionals.get(1) {
        out.push(std::ffi::OsString::from("--repo"));
        out.push(repo.clone());
    }
    out.extend(remainder);
    out
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
            "not opening a session; the tree is still at {} — inspect it with `wt tree status`, then \
             `wt llm {}` into it once you know why",
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
                     registered repo; pass one: wt go {worktree} --repo <REPO>"
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

    if let Some(repo) = repo_arg
        && !store.repos.contains_key(repo)
    {
        bail!("unknown repo '{repo}'. Known repos: {}", known_repos(store));
    }

    let scoped = |repo: Option<&str>| {
        store
            .trees
            .iter()
            .filter(|tree| repo.is_none_or(|repo| tree.repo == repo))
            .cloned()
            .collect::<Vec<_>>()
    };
    let candidates = if let Some(repo) = repo_arg {
        scoped(Some(repo))
    } else if let Some(repo) = cwd_repo {
        let in_cwd_repo = scoped(Some(repo));
        if store::resolve_optional(&in_cwd_repo, worktree)?.is_some() {
            in_cwd_repo
        } else {
            scoped(None)
        }
    } else {
        scoped(None)
    };

    match store::resolve_optional(&candidates, worktree)? {
        Some(tree) => Ok(LaunchPlan::Existing { id: tree.id }),
        None => match repo_arg {
            Some(repo) => Ok(LaunchPlan::New {
                repo: repo.to_string(),
                name: worktree.to_string(),
            }),
            None => bail!(
                "no tree matches '{worktree}'; to create one, pass --repo: wt go {worktree} --repo <REPO>"
            ),
        },
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
            "not opening a session; inspect '{pending_name}' with `wt tree status`, then `wt llm {}` \
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
                     --profile need a tree name to create one: wt go <TREE> --repo <REPO> \
                     --branch <BRANCH>"
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

    match agent {
        Agent::Pi => agent::exec_at(agent, &tree_path, &pi_launch_args(&label, args), &env),
        Agent::Claude => agent::exec_at(
            agent,
            &tree_path,
            &claude_launch_args(&label, args, color),
            &env,
        ),
        Agent::Codex => {
            agent::exec_launch_codex(&tree_path, args, &env, planter_enabled && planter_eligible)
        }
    }
}

fn pi_launch_args(name: &str, passthrough: &[String]) -> Vec<String> {
    let mut args = vec!["-n".to_string(), name.to_string()];
    args.extend(passthrough.iter().cloned());
    args
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
    if let Some(repo) = repo_filter.as_deref()
        && !store.repos.contains_key(repo)
    {
        bail!(
            "unknown repo '{repo}'. Known repos: {}",
            known_repos(&store)
        );
    }
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

/// Resolves which repo `wt gt stack` looks at, and — for the default,
/// non-`--all` view — which branch counts as "current" for the `*` marker
/// and the stack it shows.
fn stack_context(
    store: &store::Store,
    selector: Option<&str>,
    cwd: Option<&Path>,
) -> Result<(String, Option<String>)> {
    if let Some(sel) = selector {
        if let Some(t) = store::resolve_optional(&store.trees, sel)? {
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
        "not inside a registered tree or repo; pass a selector, e.g. `wt gt stack <tree>` or \
         `wt gt stack --all`"
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
    if let Some(repo) = repo_filter.as_deref()
        && !store.repos.contains_key(repo)
    {
        bail!(
            "unknown repo '{repo}'. Known repos: {}",
            known_repos(&store)
        );
    }
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
    validate_repo_filter(root, repo.as_deref())?;
    spare::refresh(root, config_path, repo.as_deref())?;
    println!("refreshing");
    Ok(())
}

/// Resolves `--repo`, else the current directory's repo, and errors
/// naming the registered repos rather than falling back to "every repo" —
/// a bare `wt repo spare drop` must never silently turn spares off everywhere.
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
    validate_repo_filter(root, Some(&repo_name))?;
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
    fn pi_launch_args_puts_generated_name_before_passthrough() {
        let args = pi_launch_args("fix login", &["-n".to_string(), "caller name".to_string()]);
        assert_eq!(args, vec!["-n", "fix login", "-n", "caller name"]);
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
            err.to_string().contains("no tree matches 'ghost'"),
            "message was: {err}"
        );
    }

    #[test]
    fn resolve_launch_ambiguous_name_across_repos_is_an_error() {
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
        assert!(err.to_string().contains("ambiguous"), "message was: {err}");
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

    fn parse(args: &[&str]) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(normalize_legacy_args(
            args.iter().map(std::ffi::OsString::from),
        ))
    }

    #[test]
    fn root_help_exposes_only_the_public_hierarchy() {
        let root = Cli::command();
        let children = visible_children(&root)
            .map(|child| child.get_name())
            .collect::<Vec<_>>();
        assert_eq!(
            children,
            ["repo", "tree", "gt", "upkeep", "llm", "go", "cd", "help"]
        );
    }

    #[test]
    fn recursive_help_is_public_deterministic_and_nested() {
        let output = recursive_help(&Cli::command(), &[]);
        assert!(output.contains("├── repo -- Manage registered base repositories"));
        assert!(output.contains("│   └── spare -- Show or manage a repository's hot spare"));
        assert!(output.contains("│       ├── refresh"));
        assert!(output.contains("├── tree -- Create, inspect, and remove worktrees"));
        assert!(output.contains("│   ├── pi -- Run Pi with its working directory set"));
        assert!(
            output.contains(
                "├── cd -- Change directory to a tree through installed shell integration"
            )
        );
        assert!(!output.contains("__provision"));
        assert!(!output.contains("launch"));
    }

    #[test]
    fn recursive_help_can_be_limited_to_a_namespace() {
        let command = Cli::command();
        let repo = command.find_subcommand("repo").unwrap();
        let output = recursive_help(repo, &["repo"]);
        assert!(output.starts_with("wt repo -- Manage registered base repositories\n"));
        assert!(output.contains("└── spare"));
        assert!(!output.contains("tree --"));
    }

    #[test]
    fn explicit_help_uses_long_help_and_excludes_hidden_commands() {
        let path = vec!["go".to_string()];
        let help = detailed_help(public_command_for_path(&path).unwrap(), &["go"]);
        assert!(help.contains("A TREE starting with @ opens a labeled scratch session"));
        assert!(help.contains("wt go @poking-around --repo monorepo"));

        let hidden = vec!["__provision".to_string()];
        assert!(public_command_for_path(&hidden).is_err());
    }

    #[test]
    fn legacy_routes_normalize_to_canonical_routes() {
        let cases = [
            (
                vec!["wt", "new", "repo", "--name", "x"],
                vec!["wt", "tree", "new", "repo", "--name", "x"],
            ),
            (vec!["wt", "ls"], vec!["wt", "tree", "ls"]),
            (vec!["wt", "path", "x"], vec!["wt", "tree", "path", "x"]),
            (vec!["wt", "name"], vec!["wt", "tree", "name"]),
            (vec!["wt", "rm", "x"], vec!["wt", "tree", "rm", "x"]),
            (vec!["wt", "status"], vec!["wt", "tree", "status"]),
            (vec!["wt", "wait"], vec!["wt", "tree", "wait"]),
            (
                vec!["wt", "env", "refresh", "x"],
                vec!["wt", "tree", "env", "x"],
            ),
            (vec!["wt", "stack"], vec!["wt", "gt", "stack"]),
            (vec!["wt", "restack"], vec!["wt", "gt", "restack"]),
            (vec!["wt", "gc"], vec!["wt", "upkeep", "gc"]),
            (vec!["wt", "doctor"], vec!["wt", "upkeep", "doctor"]),
            (vec!["wt", "sync"], vec!["wt", "repo", "sync"]),
            (vec!["wt", "spare"], vec!["wt", "repo", "spare"]),
            (vec!["wt", "claude"], vec!["wt", "llm", "claude"]),
            (vec!["wt", "codex"], vec!["wt", "llm", "codex"]),
            (vec!["wt", "adopt"], vec!["wt", "repo", "lift"]),
            (
                vec!["wt", "init", "repo", "--adopt", "/base"],
                vec!["wt", "repo", "adopt", "repo", "/base"],
            ),
            (
                vec!["wt", "init", "repo", "--adopt=/base"],
                vec!["wt", "repo", "adopt", "repo", "/base"],
            ),
            (
                vec!["wt", "launch", "tree", "repo"],
                vec!["wt", "go", "tree", "--repo", "repo"],
            ),
            (
                vec!["wt", "launch", "--branch", "feature", "tree", "repo"],
                vec!["wt", "go", "tree", "--repo", "repo", "--branch", "feature"],
            ),
            (
                vec![
                    "wt",
                    "launch",
                    "tree",
                    "--onto=base",
                    "repo",
                    "--profile=node,python",
                ],
                vec![
                    "wt",
                    "go",
                    "tree",
                    "--repo",
                    "repo",
                    "--onto=base",
                    "--profile=node,python",
                ],
            ),
        ];
        for (input, expected) in cases {
            let actual = normalize_legacy_args(input.iter().map(std::ffi::OsString::from))
                .iter()
                .map(|arg| arg.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn parser_enforces_tree_removal_and_agent_argument_constraints() {
        assert!(parse(&["wt", "tree", "rm", "tree", "--reparent-children"]).is_err());
        assert!(
            parse(&[
                "wt", "tree", "new", "repo", "--name", "tree", "--", "--model", "x"
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "wt", "tree", "new", "repo", "--name", "tree", "--pi", "--", "--model", "x"
            ])
            .is_ok()
        );
        assert!(
            parse(&[
                "wt", "tree", "new", "repo", "--name", "tree", "--codex", "--", "--model", "x"
            ])
            .is_ok()
        );
        assert!(
            parse(&[
                "wt", "repo", "lift", "repo", "--name", "tree", "--", "--model", "x"
            ])
            .is_err()
        );
        assert!(parse(&["wt", "repo", "spare", "--repo", "repo", "refresh"]).is_ok());
    }
}
