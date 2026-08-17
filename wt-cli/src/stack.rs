//! Joins Graphite's stack graph to the worktrees that hold each branch, so
//! callers can render a stack with `wt` identity — tree names instead of raw
//! worktree paths — rather than parsing `gt log --stack`'s own output.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use uuid::Uuid;

use crate::git;
use crate::graphite::{self, Graph};
use crate::store::{Repo, Store};

#[derive(Debug, Clone)]
pub enum Holder {
    Tree {
        id: Uuid,
        name: String,
        dirty: bool,
    },
    Base,
    /// A worktree git knows about that `wt` doesn't manage.
    Unregistered {
        path: PathBuf,
    },
    /// No worktree currently has this branch checked out.
    None,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub branch: String,
    pub parent: Option<String>,
    pub needs_restack: Option<bool>,
    pub pr_number: Option<u64>,
    pub pr_state: Option<String>,
    pub pr_review_decision: Option<String>,
    pub pr_draft: Option<bool>,
    pub holder: Holder,
}

impl Entry {
    /// Graphite has one annotation slot per branch, and `(merged)` masks
    /// `(needs restack)` — so a branch whose pull request is already merged
    /// or closed is done, whatever its actual git shape says: `wt stack`
    /// hides it by default and the restack planner skips it outright.
    pub fn is_merged_or_closed(&self) -> bool {
        matches!(self.pr_state.as_deref(), Some("MERGED") | Some("CLOSED"))
    }
}

pub struct Stacks {
    pub graph: Graph,
    entries: HashMap<String, Entry>,
}

impl Stacks {
    /// `branches` in the graph's bottom-up order. Every name the graph
    /// knows about got an entry in `load`, so this only drops names that
    /// aren't in the graph at all.
    pub fn ordered(&self, branches: &[String]) -> Vec<&Entry> {
        self.graph
            .topo_order(branches)
            .into_iter()
            .filter_map(|b| self.entries.get(&b))
            .collect()
    }

    pub fn get(&self, branch: &str) -> Option<&Entry> {
        self.entries.get(branch)
    }
}

/// `None` means Graphite has no readable stack graph for this repo — every
/// caller must treat that the same as "nothing to show", never as an error.
pub fn load(repo_name: &str, repo: &Repo, store: &Store) -> Result<Option<Stacks>> {
    let common_dir = git::common_dir(&repo.base)?;
    if !graphite::available(&common_dir) {
        return Ok(None);
    }
    let graph = graphite::graph(&common_dir)?;
    let worktrees = git::worktree_branches(&repo.base)?;
    let base = std::fs::canonicalize(&repo.base).unwrap_or_else(|_| repo.base.clone());

    let mut entries: HashMap<String, Entry> = HashMap::new();
    for (path, branch) in &worktrees {
        let Some(branch) = branch else { continue };
        let Some(node) = graph.get(branch) else {
            continue;
        };
        let holder = if *path == base {
            Holder::Base
        } else if let Some(t) = store
            .trees
            .iter()
            .find(|t| t.repo == repo_name && &t.path == path)
        {
            Holder::Tree {
                id: t.id,
                name: t.name.clone(),
                dirty: git::is_dirty(path).unwrap_or(false),
            }
        } else {
            Holder::Unregistered { path: path.clone() }
        };
        entries.insert(branch.clone(), entry_from(branch, node, holder));
    }
    // A branch Graphite tracks but nobody currently has checked out still
    // belongs on the graph, just with no holder.
    for branch in graph.branch_names() {
        entries.entry(branch.to_string()).or_insert_with(|| {
            let node = graph.get(branch).expect("came from graph.branch_names()");
            entry_from(branch, node, Holder::None)
        });
    }

    Ok(Some(Stacks { graph, entries }))
}

fn entry_from(branch: &str, node: &graphite::Node, holder: Holder) -> Entry {
    Entry {
        branch: branch.to_string(),
        parent: node.parent.clone(),
        needs_restack: node.needs_restack,
        pr_number: node.pr_number,
        pr_state: node.pr_state.clone(),
        pr_review_decision: node.pr_review_decision.clone(),
        pr_draft: node.pr_draft,
        holder,
    }
}

/// Where one branch lives, without the dirty flag `Holder::Tree` carries —
/// `position` never spawns the extra `git status` per neighbor that would
/// take to know it, so this leaves it out rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeighborHolder {
    Tree { name: String },
    Base,
    Unregistered,
    None,
}

/// A branch's immediate Graphite neighbors — its parent and children — each
/// paired with who holds it. `None` for either side when there is none.
pub struct Position {
    pub parent: Option<(String, NeighborHolder)>,
    pub children: Vec<(String, NeighborHolder)>,
}

/// The minimal join a caller needs to place one branch in its stack: no
/// pull-request read, no needs-restack computation (`graphite::graph_light`
/// skips both), and the worktree scan only runs once there's a neighbor
/// worth resolving. Built for `wt __session-context`, which pays this cost
/// on every prompt and can't afford `load`'s whole-graph price.
///
/// `None` means Graphite has no readable stack graph for this repo, or
/// doesn't track `branch` at all — both "nothing to show", never an error.
pub fn position(
    repo_name: &str,
    repo: &Repo,
    store: &Store,
    branch: &str,
) -> Result<Option<Position>> {
    let common_dir = git::common_dir(&repo.base)?;
    if !graphite::available(&common_dir) {
        return Ok(None);
    }
    let graph = graphite::graph_light(&common_dir)?;
    let Some(node) = graph.get(branch) else {
        return Ok(None);
    };
    if node.parent.is_none() && node.children.is_empty() {
        return Ok(Some(Position {
            parent: None,
            children: Vec::new(),
        }));
    }

    let worktrees = git::worktree_branches(&repo.base)?;
    let base = std::fs::canonicalize(&repo.base).unwrap_or_else(|_| repo.base.clone());
    let path_for: HashMap<&str, &PathBuf> = worktrees
        .iter()
        .filter_map(|(path, b)| b.as_deref().map(|b| (b, path)))
        .collect();
    let holder_of = |b: &str| -> NeighborHolder {
        match path_for.get(b) {
            None => NeighborHolder::None,
            Some(&path) if *path == base => NeighborHolder::Base,
            Some(&path) => store
                .trees
                .iter()
                .find(|t| t.repo == repo_name && &t.path == path)
                .map(|t| NeighborHolder::Tree {
                    name: t.name.clone(),
                })
                .unwrap_or(NeighborHolder::Unregistered),
        }
    };

    let parent = node.parent.clone().map(|p| {
        let holder = holder_of(&p);
        (p, holder)
    });
    let children = node
        .children
        .iter()
        .map(|c| (c.clone(), holder_of(c)))
        .collect();
    Ok(Some(Position { parent, children }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use chrono::Utc;
    use uuid::Uuid;

    use crate::store::{Tree, TreeState};

    fn git(args: &[&str], cwd: &Path) {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn sqlite(db: &Path, sql: &str) {
        let out = Command::new("/usr/bin/sqlite3")
            .arg(db)
            .arg(sql)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "sqlite3 {sql} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn sample_tree(name: &str, branch: &str, path: PathBuf) -> Tree {
        Tree {
            id: Uuid::now_v7(),
            repo: "r".into(),
            name: name.into(),
            branch: branch.into(),
            path,
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

    fn sample_repo(base: PathBuf) -> Repo {
        Repo {
            base,
            last_fetch: None,
        }
    }

    /// A base on `master`, a registered tree on `a`, an unregistered
    /// worktree on `b`, and `c` tracked by Graphite but checked out
    /// nowhere — one of each kind of holder `load` distinguishes.
    fn fixture() -> (PathBuf, Repo, Vec<Tree>) {
        let dir = std::env::temp_dir().join(format!("wt-stack-test-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        git(&["init", "-q", "-b", "master"], &base);
        git(&["config", "user.email", "t@t"], &base);
        git(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);
        git(&["branch", "a"], &base);
        git(&["branch", "b"], &base);
        git(&["branch", "c"], &base);

        let tree_a = dir.join("tree-a");
        git(&["worktree", "add", tree_a.to_str().unwrap(), "a"], &base);
        let tree_b = dir.join("tree-b-unregistered");
        git(&["worktree", "add", tree_b.to_str().unwrap(), "b"], &base);

        let common_dir = base.join(".git");
        let db = common_dir.join(".graphite_metadata.db");
        sqlite(
            &db,
            "CREATE TABLE branch_metadata (\
             branch_name TEXT PRIMARY KEY, parent_branch_name TEXT, \
             parent_branch_revision TEXT, last_submitted_version TEXT, state TEXT, \
             children TEXT, branch_revision TEXT, validation_result TEXT, \
             parent_head_revision TEXT);",
        );
        sqlite(
            &db,
            "INSERT INTO branch_metadata (branch_name, parent_branch_name) VALUES \
             ('master', NULL), ('a', 'master'), ('b', 'a'), ('c', 'b');",
        );

        let repo = sample_repo(base.clone());
        let trees = vec![sample_tree(
            "tree-a",
            "a",
            fs::canonicalize(&tree_a).unwrap(),
        )];
        (dir, repo, trees)
    }

    fn store_with(repo_name: &str, repo: &Repo, trees: Vec<Tree>) -> Store {
        let mut store = Store::default();
        store.repos.insert(repo_name.to_string(), repo.clone());
        store.trees = trees;
        store
    }

    #[test]
    fn load_distinguishes_every_kind_of_holder() {
        let (dir, repo, trees) = fixture();
        let store = store_with("r", &repo, trees);

        let stacks = load("r", &repo, &store)
            .unwrap()
            .expect("graphite available");
        let branches = vec![
            "master".to_string(),
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
        ];
        let by_branch: HashMap<&str, &Entry> = stacks
            .ordered(&branches)
            .into_iter()
            .map(|e| (e.branch.as_str(), e))
            .collect();

        assert!(matches!(by_branch["master"].holder, Holder::Base));
        assert!(matches!(by_branch["a"].holder, Holder::Tree { .. }));
        assert!(matches!(by_branch["b"].holder, Holder::Unregistered { .. }));
        assert!(matches!(by_branch["c"].holder, Holder::None));

        if let Holder::Tree { name, .. } = &by_branch["a"].holder {
            assert_eq!(name, "tree-a");
        }

        fs::remove_dir_all(&dir).ok();
    }

    fn entry_with_pr_state(state: Option<&str>) -> Entry {
        Entry {
            branch: "josh/b".into(),
            parent: Some("master".into()),
            needs_restack: None,
            pr_number: None,
            pr_state: state.map(str::to_string),
            pr_review_decision: None,
            pr_draft: None,
            holder: Holder::None,
        }
    }

    #[test]
    fn only_a_known_merged_or_closed_pr_hides_a_branch() {
        assert!(entry_with_pr_state(Some("MERGED")).is_merged_or_closed());
        assert!(entry_with_pr_state(Some("CLOSED")).is_merged_or_closed());
        assert!(!entry_with_pr_state(Some("OPEN")).is_merged_or_closed());
        // An unreadable `.graphite_pr_info` leaves every state `None`; hiding on
        // that would empty the default view of branches that are still live.
        assert!(!entry_with_pr_state(None).is_merged_or_closed());
    }

    #[test]
    fn load_returns_none_when_graphite_is_unavailable() {
        let dir = std::env::temp_dir().join(format!("wt-stack-test-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        git(&["init", "-q", "-b", "master"], &base);
        git(&["config", "user.email", "t@t"], &base);
        git(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);
        // No `.graphite_metadata.db` written — this repo never ran `gt`.

        let repo = sample_repo(base);
        let store = store_with("r", &repo, Vec::new());

        assert!(load("r", &repo, &store).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn position_resolves_a_base_parent_and_an_unregistered_child() {
        let (dir, repo, trees) = fixture();
        let store = store_with("r", &repo, trees);

        let pos = position("r", &repo, &store, "a").unwrap().unwrap();
        assert_eq!(
            pos.parent,
            Some(("master".to_string(), NeighborHolder::Base))
        );
        assert_eq!(
            pos.children,
            vec![("b".to_string(), NeighborHolder::Unregistered)]
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn position_resolves_a_tree_parent_and_an_unheld_child() {
        let (dir, repo, trees) = fixture();
        let store = store_with("r", &repo, trees);

        let pos = position("r", &repo, &store, "b").unwrap().unwrap();
        assert_eq!(
            pos.parent,
            Some((
                "a".to_string(),
                NeighborHolder::Tree {
                    name: "tree-a".to_string()
                }
            ))
        );
        assert_eq!(pos.children, vec![("c".to_string(), NeighborHolder::None)]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn position_is_none_for_a_branch_graphite_never_tracked() {
        let (dir, repo, trees) = fixture();
        let store = store_with("r", &repo, trees);

        assert!(
            position("r", &repo, &store, "never-tracked")
                .unwrap()
                .is_none()
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn position_returns_none_when_graphite_is_unavailable() {
        let dir = std::env::temp_dir().join(format!("wt-stack-test-{}", Uuid::now_v7()));
        let base = dir.join("base");
        fs::create_dir_all(&base).unwrap();
        git(&["init", "-q", "-b", "master"], &base);
        git(&["config", "user.email", "t@t"], &base);
        git(&["config", "user.name", "t"], &base);
        fs::write(base.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &base);
        git(&["commit", "-qm", "init"], &base);

        let repo = sample_repo(base);
        let store = store_with("r", &repo, Vec::new());

        assert!(position("r", &repo, &store, "master").unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }
}
