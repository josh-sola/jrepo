use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::stack::{self, NeighborHolder};
use crate::store;

/// Backs `hooks/session-context.sh`. Prints a few lines of plain-English
/// context for the resolved directory, or nothing if it isn't a registered
/// tree or base — the hook script treats empty output and any error the
/// same way, so this never needs to distinguish "not applicable" from
/// "couldn't tell".
pub fn session_context(root: &Path, cwd: Option<PathBuf>) -> Result<()> {
    let cwd = match cwd {
        Some(c) => c,
        None => std::env::current_dir()?,
    };
    let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
    let store = store::load(root)?;

    if let Some(tree) = store
        .trees
        .iter()
        .filter(|t| cwd.starts_with(&t.path))
        .max_by_key(|t| t.path.components().count())
    {
        let branch = store::live_branch(tree).unwrap_or_else(|| tree.branch.clone());
        let mut lines = vec![format!(
            "This directory is the wt tree \"{}\" (branch {}) of repo {}.",
            tree.name, branch, tree.repo
        )];
        let plans = tree.path.join("plans");
        if plans.is_dir() {
            lines.push(format!(
                "Plans live at {} — read and write them there. Every tree of this repo shares \
                 that directory and it survives teardown, so plans belong nowhere else.",
                plans.display()
            ));
        }
        if let Some(repo) = store.repos.get(&tree.repo) {
            // Any failure here — no Graphite, an unreadable database, a
            // branch it doesn't track — just means nothing more to add.
            // The context above still stands on its own.
            if let Ok(Some(position)) = stack::position(&tree.repo, repo, &store, &branch) {
                lines.extend(stack_position_lines(&position));
            }
        }
        println!("{}", lines.join("\n"));
        return Ok(());
    }

    if let Some((repo_name, _)) = store.repos.iter().find(|(_, r)| cwd.starts_with(&r.base)) {
        println!(
            "This directory is {repo_name}'s base checkout. It stays on trunk; do not edit or \
             commit here. Run `wt tree new {repo_name} --name \"...\"` to start work in a disposable \
             tree."
        );
    }

    Ok(())
}

fn holder_desc(h: &NeighborHolder) -> String {
    match h {
        NeighborHolder::Tree { name } => format!("tree \"{name}\""),
        NeighborHolder::Base => "the repo's base checkout".to_string(),
        NeighborHolder::Unregistered => "a worktree wt doesn't manage".to_string(),
        NeighborHolder::None => "no worktree right now".to_string(),
    }
}

/// Plain-English lines placing this branch in its Graphite stack: what it's
/// built on and who holds that, what's built on it and who holds those, and
/// — the payoff for a multi-agent setup — an explicit warning not to restack
/// or rebase a branch that another tree owns from in here.
fn stack_position_lines(position: &stack::Position) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some((parent, holder)) = &position.parent {
        lines.push(format!(
            "Below this branch in the stack: '{parent}', held by {}.",
            holder_desc(holder)
        ));
        if let NeighborHolder::Tree { name } = holder {
            lines.push(format!(
                "This tree is mid-stack: '{parent}' belongs to tree \"{name}\", not this one. \
                 Don't rebase or restack '{parent}' from here — do that in tree \"{name}\", or \
                 run `wt gt restack` to walk the whole stack in the right order."
            ));
        }
    }

    if !position.children.is_empty() {
        let named = position
            .children
            .iter()
            .map(|(c, h)| format!("'{c}' ({})", holder_desc(h)))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "Stacked on top of this branch: {named}. Changes here can require a restack there \
             too."
        ));
    }

    lines
}
