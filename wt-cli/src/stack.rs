//! Builds wt's own stack graph from tree records — branch and parent branch,
//! recorded at creation — and joins it to the worktrees that hold each
//! branch, so callers can render a stack with `wt` identity — tree names
//! instead of raw worktree paths — rather than parsing `gt log --stack`'s
//! own output or Graphite's db.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// The tree's own stored flag, set by a walk or `wt sync` when this
    /// branch was skipped or its parent moved, and cleared once it
    /// restacks — independent of `needs_restack`, which recomputes fresh
    /// from git each time and can disagree with it in either direction.
    pub pending_restack: bool,
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

    /// Either source alone is enough to show the marker: the stored flag
    /// and the freshly derived check can disagree, and neither is more
    /// authoritative than the other.
    pub fn shows_needs_restack(&self) -> bool {
        self.pending_restack || self.needs_restack == Some(true)
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

/// The graph over exactly the trees `wt` itself created for `repo_name`: one
/// node per `Tree.branch`, parented on `Tree.parent_branch` when that names
/// another node — trunk and any branch with no tree of its own are never
/// nodes, so they surface as roots rather than members of the stacks above
/// them. A tree created before wt recorded parent edges has no recorded
/// parent and so shows as its own root.
fn wt_graph(repo_name: &str, store: &Store) -> Graph {
    Graph::from_edges(
        store
            .trees
            .iter()
            .filter(|t| t.repo == repo_name && !t.spare)
            .map(|t| (t.branch.clone(), t.parent_branch.clone())),
    )
}

/// A branch needs a restack when its parent's current head is not an
/// ancestor of it, unless the branch's own pull request is already merged
/// or closed — Graphite's `(merged)` annotation masks `(needs restack)`, so
/// a merged branch is never reported as needing one regardless of its
/// actual git shape.
fn needs_restack(
    repo_base: &Path,
    parent: Option<&str>,
    branch: &str,
    heads: &HashMap<String, String>,
    parent_revision: Option<&str>,
    pr_infos: Option<&HashMap<String, graphite::PrInfo>>,
) -> Option<bool> {
    let parent = parent?;
    let parent_head = heads.get(parent)?;
    let branch_head = heads.get(branch)?;

    // If the parent hasn't moved since this branch last recorded its fork
    // point, that fork point is trivially still on the parent's line, and
    // therefore still an ancestor of this branch too — no need to pay for
    // `merge-base` on the branches this is true for. A stale or missing
    // `parent_revision` (no restack has recorded one yet) just means this
    // prefilter misses and the real check below runs instead.
    let is_ancestor = if parent_revision == Some(parent_head.as_str()) {
        true
    } else {
        git::is_ancestor(repo_base, parent_head, branch_head).ok()?
    };

    if is_ancestor {
        return Some(false);
    }
    let masked = pr_infos?
        .get(branch)
        .is_some_and(|pr| matches!(pr.state.as_str(), "MERGED" | "CLOSED"));
    Some(!masked)
}

/// Where `path` is held, resolved against `wt`'s own tree registry: `wt`'s
/// base checkout, one of its trees, or a worktree it doesn't manage at all.
pub(crate) fn holder_for(repo_name: &str, store: &Store, base: &Path, path: &Path) -> Holder {
    if path == base {
        Holder::Base
    } else if let Some(t) = store
        .trees
        .iter()
        .find(|t| t.repo == repo_name && t.path == path)
    {
        Holder::Tree {
            id: t.id,
            name: t.name.clone(),
            dirty: git::is_dirty(path).unwrap_or(false),
        }
    } else {
        Holder::Unregistered {
            path: path.to_path_buf(),
        }
    }
}

/// `None` when `repo_name` has no trees of its own — nothing for a stack
/// view to show, so every caller must treat that the same as "nothing to
/// show", never as an error.
pub fn load(repo_name: &str, repo: &Repo, store: &Store) -> Result<Option<Stacks>> {
    let has_trees = store.trees.iter().any(|t| t.repo == repo_name && !t.spare);
    if !has_trees {
        return Ok(None);
    }

    let mut graph = wt_graph(repo_name, store);
    let common_dir = git::common_dir(&repo.base)?;

    let pr_infos = graphite::read_pr_info(&common_dir);
    if let Some(prs) = &pr_infos {
        for (branch, pr) in prs {
            if let Some(node) = graph.get_mut(branch) {
                node.pr_number = Some(pr.pr_number);
                node.pr_state = Some(pr.state.clone());
                node.pr_review_decision = pr.review_decision.clone();
                node.pr_draft = Some(pr.is_draft);
            }
        }
    } else {
        // No sidecar to read at all — the number `wt submit` recorded is
        // stale-proof (a PR number never changes) even though its state
        // isn't available this way.
        for t in store
            .trees
            .iter()
            .filter(|t| t.repo == repo_name && !t.spare)
        {
            if let Some(pr_number) = t.pr_number
                && let Some(node) = graph.get_mut(&t.branch)
            {
                node.pr_number = Some(pr_number);
            }
        }
    }

    if let Ok(heads) = git::live_heads(&repo.base) {
        let branches: Vec<String> = graph.branch_names().map(str::to_string).collect();
        for branch in &branches {
            let parent = graph.get(branch).and_then(|n| n.parent.clone());
            let parent_revision = store
                .trees
                .iter()
                .find(|t| t.repo == repo_name && &t.branch == branch)
                .and_then(|t| t.parent_revision.as_deref());
            let needs = needs_restack(
                &repo.base,
                parent.as_deref(),
                branch,
                &heads,
                parent_revision,
                pr_infos.as_ref(),
            );
            if let Some(node) = graph.get_mut(branch) {
                node.needs_restack = needs;
            }
        }
    }

    let worktrees = git::worktree_branches(&repo.base)?;
    let base = std::fs::canonicalize(&repo.base).unwrap_or_else(|_| repo.base.clone());

    let mut entries: HashMap<String, Entry> = HashMap::new();
    for (path, branch) in &worktrees {
        let Some(branch) = branch else { continue };
        let Some(node) = graph.get(branch) else {
            continue;
        };
        let holder = holder_for(repo_name, store, &base, path);
        let pending = pending_restack(repo_name, store, branch);
        entries.insert(branch.clone(), entry_from(branch, node, holder, pending));
    }
    // A tree's branch that nobody currently has checked out still belongs
    // on the graph, just with no holder.
    for branch in graph.branch_names() {
        entries.entry(branch.to_string()).or_insert_with(|| {
            let node = graph.get(branch).expect("came from graph.branch_names()");
            entry_from(
                branch,
                node,
                Holder::None,
                pending_restack(repo_name, store, branch),
            )
        });
    }

    Ok(Some(Stacks { graph, entries }))
}

fn pending_restack(repo_name: &str, store: &Store, branch: &str) -> bool {
    store
        .trees
        .iter()
        .find(|t| t.repo == repo_name && t.branch == branch)
        .is_some_and(|t| t.pending_restack)
}

fn entry_from(branch: &str, node: &graphite::Node, holder: Holder, pending_restack: bool) -> Entry {
    Entry {
        branch: branch.to_string(),
        parent: node.parent.clone(),
        needs_restack: node.needs_restack,
        pending_restack,
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
/// pull-request read, no needs-restack computation, and the worktree scan
/// only runs once there's a neighbor worth resolving. `wt_graph` is built
/// from `store` already in memory, so unlike `load` this needs no db or
/// sidecar read at all. Built for `wt __session-context`, which pays this
/// cost on every prompt and can't afford `load`'s whole-graph price.
///
/// `None` when `branch` has no tree of its own in `repo_name` — nothing to
/// show, never an error.
pub fn position(
    repo_name: &str,
    repo: &Repo,
    store: &Store,
    branch: &str,
) -> Result<Option<Position>> {
    let graph = wt_graph(repo_name, store);
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

    fn sample_tree(name: &str, branch: &str, parent_branch: Option<&str>, path: PathBuf) -> Tree {
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
            parent_branch: parent_branch.map(str::to_string),
            parent_revision: None,
            pending_restack: false,
            pr_number: None,
            spare: false,
        }
    }

    fn sample_repo(base: PathBuf) -> Repo {
        Repo {
            base,
            last_fetch: None,
        }
    }

    /// Trees on `a` (root), `b` (parent `a`, registered at a path that has
    /// drifted from where `b` is actually checked out), and `c` (parent `b`,
    /// no worktree at all) — one of each kind of holder `load` distinguishes
    /// now that a tree's own records are what makes a branch a graph node.
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
        let tree_b = dir.join("tree-b-actual-checkout");
        git(&["worktree", "add", tree_b.to_str().unwrap(), "b"], &base);

        let repo = sample_repo(base.clone());
        let trees = vec![
            sample_tree("tree-a", "a", None, fs::canonicalize(&tree_a).unwrap()),
            sample_tree(
                "tree-b",
                "b",
                Some("a"),
                dir.join("tree-b-stale-registration"),
            ),
            sample_tree(
                "tree-c",
                "c",
                Some("b"),
                dir.join("tree-c-never-checked-out"),
            ),
        ];
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

        let stacks = load("r", &repo, &store).unwrap().expect("r has trees");
        let branches = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let by_branch: HashMap<&str, &Entry> = stacks
            .ordered(&branches)
            .into_iter()
            .map(|e| (e.branch.as_str(), e))
            .collect();

        assert!(matches!(by_branch["a"].holder, Holder::Tree { .. }));
        assert!(matches!(by_branch["b"].holder, Holder::Unregistered { .. }));
        assert!(matches!(by_branch["c"].holder, Holder::None));

        if let Holder::Tree { name, .. } = &by_branch["a"].holder {
            assert_eq!(name, "tree-a");
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_falls_back_to_the_stored_pr_number_when_the_sidecar_is_unreadable() {
        let (dir, repo, mut trees) = fixture();
        // No `.graphite_pr_info` written for this repo at all.
        trees[0].pr_number = Some(42);
        let store = store_with("r", &repo, trees);

        let stacks = load("r", &repo, &store).unwrap().expect("r has trees");
        assert_eq!(stacks.get("a").unwrap().pr_number, Some(42));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_prefers_the_sidecar_over_the_stored_pr_number_when_both_exist() {
        let (dir, repo, mut trees) = fixture();
        trees[0].pr_number = Some(42);
        write_pr_info(&repo.base, "a", "OPEN");
        let store = store_with("r", &repo, trees);

        let stacks = load("r", &repo, &store).unwrap().expect("r has trees");
        assert_eq!(stacks.get("a").unwrap().pr_number, Some(1));

        fs::remove_dir_all(&dir).ok();
    }

    fn entry_with_pr_state(state: Option<&str>) -> Entry {
        Entry {
            branch: "josh/b".into(),
            parent: Some("master".into()),
            needs_restack: None,
            pending_restack: false,
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
    fn load_returns_none_when_the_repo_has_no_trees() {
        let repo = sample_repo(PathBuf::from("/nonexistent-base"));
        let store = store_with("r", &repo, Vec::new());

        assert!(load("r", &repo, &store).unwrap().is_none());
    }

    #[test]
    fn position_resolves_no_parent_and_an_unregistered_child() {
        let (dir, repo, trees) = fixture();
        let store = store_with("r", &repo, trees);

        let pos = position("r", &repo, &store, "a").unwrap().unwrap();
        assert_eq!(pos.parent, None);
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
    fn position_is_none_for_a_branch_with_no_tree() {
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
    fn position_returns_none_when_the_repo_has_no_trees() {
        let repo = sample_repo(PathBuf::from("/nonexistent-base"));
        let store = store_with("r", &repo, Vec::new());

        assert!(position("r", &repo, &store, "master").unwrap().is_none());
    }

    fn git_rev_parse(dir: &Path, rev: &str) -> String {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", rev])
                .current_dir(dir)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    /// `master` and `child` (forked from it), where `master` has since moved
    /// past the commit `child` forked from — the shape that requires an
    /// actual `merge-base` check, not just the recorded-revision prefilter.
    fn fixture_with_stale_child() -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("wt-stack-restack-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        git(&["init", "-q", "-b", "master"], &dir);
        git(&["config", "user.email", "t@t"], &dir);
        git(&["config", "user.name", "t"], &dir);
        fs::write(dir.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &dir);
        git(&["commit", "-qm", "c0"], &dir);
        let fork_point = git_rev_parse(&dir, "HEAD");

        git(&["checkout", "-qb", "child"], &dir);
        fs::write(dir.join("child.txt"), "1\n").unwrap();
        git(&["add", "-A"], &dir);
        git(&["commit", "-qm", "c1"], &dir);

        git(&["checkout", "-q", "master"], &dir);
        fs::write(dir.join("f.txt"), "1\n").unwrap();
        git(&["add", "-A"], &dir);
        git(&["commit", "-qm", "c2"], &dir);
        (dir, fork_point)
    }

    fn write_pr_info(dir: &Path, branch: &str, state: &str) {
        fs::write(
            dir.join(".git").join(".graphite_pr_info"),
            format!(
                r#"{{"prInfos": [{{"headRefName": "{branch}", "prNumber": 1,
                 "state": "{state}", "reviewDecision": null, "isDraft": false}}]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn needs_restack_is_true_when_parent_moved_past_the_fork_point_and_the_pr_is_open() {
        let (dir, fork_point) = fixture_with_stale_child();
        write_pr_info(&dir, "child", "OPEN");
        let heads = git::live_heads(&dir).unwrap();
        let pr_infos = graphite::read_pr_info(&dir.join(".git"));

        let needs = needs_restack(
            &dir,
            Some("master"),
            "child",
            &heads,
            Some(fork_point.as_str()),
            pr_infos.as_ref(),
        );
        assert_eq!(needs, Some(true));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_restack_is_masked_by_a_merged_pr() {
        let (dir, fork_point) = fixture_with_stale_child();
        write_pr_info(&dir, "child", "MERGED");
        let heads = git::live_heads(&dir).unwrap();
        let pr_infos = graphite::read_pr_info(&dir.join(".git"));

        let needs = needs_restack(
            &dir,
            Some("master"),
            "child",
            &heads,
            Some(fork_point.as_str()),
            pr_infos.as_ref(),
        );
        assert_eq!(needs, Some(false));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_restack_is_unknown_without_readable_pr_info() {
        let (dir, fork_point) = fixture_with_stale_child();
        // No `.graphite_pr_info` written — masking can't be ruled out.
        let heads = git::live_heads(&dir).unwrap();

        let needs = needs_restack(
            &dir,
            Some("master"),
            "child",
            &heads,
            Some(fork_point.as_str()),
            None,
        );
        assert_eq!(needs, None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_restack_is_false_when_the_recorded_fork_point_is_still_current() {
        // `child` freshly branched off `master`'s current tip: the prefilter
        // alone settles this without a `merge-base` call, so it's correct
        // even with no PR info to fall back on.
        let dir = std::env::temp_dir().join(format!("wt-stack-restack-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        git(&["init", "-q", "-b", "master"], &dir);
        git(&["config", "user.email", "t@t"], &dir);
        git(&["config", "user.name", "t"], &dir);
        fs::write(dir.join("f.txt"), "0\n").unwrap();
        git(&["add", "-A"], &dir);
        git(&["commit", "-qm", "c0"], &dir);
        let head = git_rev_parse(&dir, "HEAD");
        git(&["checkout", "-qb", "child"], &dir);
        fs::write(dir.join("child.txt"), "1\n").unwrap();
        git(&["add", "-A"], &dir);
        git(&["commit", "-qm", "c1"], &dir);

        let heads = git::live_heads(&dir).unwrap();
        let needs = needs_restack(
            &dir,
            Some("master"),
            "child",
            &heads,
            Some(head.as_str()),
            None,
        );
        assert_eq!(needs, Some(false));
        fs::remove_dir_all(&dir).ok();
    }
}
