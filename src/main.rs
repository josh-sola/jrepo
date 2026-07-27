mod git;
mod repo;
mod store;
mod tree;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

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
        Command::Rm { selector, force } => tree::rm_tree(root, &selector, force),
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
        let state = match t.state {
            store::TreeState::Provisioning => "provisioning",
            store::TreeState::Ready => "ready",
            store::TreeState::Failed => "failed",
        };
        let short_id = &t.id.to_string()[..8];
        println!(
            "{:<24} {:<12} {:<28} {:<12} {:<8} {}",
            t.name,
            t.repo,
            t.branch,
            state,
            short_id,
            if dirty { "dirty" } else { "" }
        );
    }
    Ok(())
}
