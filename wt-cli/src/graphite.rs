//! Read-only access to Graphite's own records: a private SQLite file plus a
//! JSON sidecar, both in the git common dir. `wt`'s own state is the primary
//! source for stack shape; these survive for two consumers: the
//! delete-guard union in `tree.rs` (a branch with no wt tree of its own,
//! tracked by `gt` before wt recorded parent edges or created out of band,
//! exists only here), and the drift findings `wt upkeep doctor` reports.
//! `wt` only ever reads them — every mutation goes through `gt`. Any
//! missing or unexpected piece (the `sqlite3` binary, either file, a schema
//! change) degrades to "no stack info" rather than an error.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const SQLITE_BIN: &str = "/usr/bin/sqlite3";
const DB_FILE: &str = ".graphite_metadata.db";
const PR_INFO_FILE: &str = ".graphite_pr_info";
const REQUIRED_COLUMNS: [&str; 3] = ["branch_name", "parent_branch_name", "state"];

fn db_path(git_common_dir: &Path) -> PathBuf {
    git_common_dir.join(DB_FILE)
}

/// True only if `sqlite3` is present, the database file exists, and
/// `branch_metadata` has the columns this module reads. Checked with a
/// single query so a Graphite schema change is caught here, before `graph`
/// assumes a shape that no longer exists.
pub fn available(git_common_dir: &Path) -> bool {
    if !Path::new(SQLITE_BIN).exists() || !db_path(git_common_dir).exists() {
        return false;
    }
    has_expected_schema(git_common_dir).unwrap_or(false)
}

fn has_expected_schema(git_common_dir: &Path) -> Result<bool> {
    // `pragma_table_info` on a table that doesn't exist returns zero rows
    // rather than an error, so table existence is checked explicitly in the
    // same query rather than inferred from the columns coming back empty.
    let json = query_json(
        git_common_dir,
        "SELECT name FROM sqlite_master WHERE type='table' AND name='branch_metadata' \
         UNION ALL SELECT name FROM pragma_table_info('branch_metadata')",
    )?;
    #[derive(Deserialize)]
    struct NameRow {
        name: String,
    }
    let rows: Vec<NameRow> = serde_json::from_str(&json).context("parsing schema probe output")?;
    let names: HashSet<String> = rows.into_iter().map(|r| r.name).collect();
    Ok(names.contains("branch_metadata") && REQUIRED_COLUMNS.iter().all(|c| names.contains(*c)))
}

fn query_json(git_common_dir: &Path, sql: &str) -> Result<String> {
    let path = db_path(git_common_dir);
    let out = Command::new(SQLITE_BIN)
        .args(["-readonly", "-json"])
        .arg(&path)
        .arg(sql)
        .output()
        .with_context(|| format!("running sqlite3 against {}", path.display()))?;
    if !out.status.success() {
        bail!(
            "sqlite3 query against {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // `sqlite3 -json` prints nothing at all for a zero-row result, not `[]`.
    if stdout.trim().is_empty() {
        return Ok("[]".to_string());
    }
    Ok(stdout)
}

#[derive(Debug, Deserialize)]
struct BranchRow {
    branch_name: String,
    parent_branch_name: Option<String>,
}

/// Graphite's GitHub mirror for one branch's pull request, read from
/// `.graphite_pr_info`. `state` is `OPEN`, `MERGED`, or `CLOSED`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PrInfo {
    #[serde(rename = "headRefName")]
    pub(crate) head_ref_name: String,
    #[serde(rename = "prNumber")]
    pub(crate) pr_number: u64,
    pub(crate) state: String,
    #[serde(rename = "reviewDecision")]
    pub(crate) review_decision: Option<String>,
    #[serde(rename = "isDraft")]
    pub(crate) is_draft: bool,
}

#[derive(Debug, Deserialize)]
struct PrInfoFile {
    #[serde(rename = "prInfos")]
    pr_infos: Vec<PrInfo>,
}

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub parent: Option<String>,
    pub children: Vec<String>,
    /// A GitHub-merged or closed PR is never reported as needing a
    /// restack — Graphite has one annotation slot per branch and `(merged)`
    /// masks `(needs restack)` — so this is `None` whenever PR state can't
    /// be read, not just whenever ancestry can't be.
    pub needs_restack: Option<bool>,
    pub pr_number: Option<u64>,
    pub pr_state: Option<String>,
    pub pr_review_decision: Option<String>,
    pub pr_draft: Option<bool>,
}

#[derive(Debug, Default)]
pub struct Graph {
    nodes: BTreeMap<String, Node>,
}

fn fetch_rows(git_common_dir: &Path) -> Result<Vec<BranchRow>> {
    let json = query_json(
        git_common_dir,
        "SELECT branch_name, parent_branch_name FROM branch_metadata",
    )?;
    serde_json::from_str(&json).context("parsing branch_metadata")
}

/// Graphite's own parent/child edges, with no pull-request read and no
/// needs-restack computation — `wt`'s own state answers those. This
/// survives for the delete-guard union in `tree.rs` and doctor's drift
/// findings: a branch `gt` tracked before wt recorded parent edges, or one
/// created out of band, can exist only in this db, never in a wt tree.
pub fn graph_light(git_common_dir: &Path) -> Result<Graph> {
    let rows = fetch_rows(git_common_dir)?;
    Ok(Graph::from_edges(
        rows.into_iter()
            .map(|r| (r.branch_name, r.parent_branch_name)),
    ))
}

/// `.graphite_pr_info` is a plain JSON sidecar `gt` writes after submit,
/// independent of the sqlite db's schema — so this is read on its own,
/// never gated on `available`.
pub(crate) fn read_pr_info(git_common_dir: &Path) -> Option<HashMap<String, PrInfo>> {
    let bytes = std::fs::read(git_common_dir.join(PR_INFO_FILE)).ok()?;
    let file: PrInfoFile = serde_json::from_slice(&bytes).ok()?;
    Some(
        file.pr_infos
            .into_iter()
            .map(|pr| (pr.head_ref_name.clone(), pr))
            .collect(),
    )
}

/// `wt go '#<N>'`'s local-first lookup: the sidecar keys on head branch, so
/// finding a PR by number means scanning its values rather than exposing the
/// map's shape to the caller.
pub(crate) fn pr_branch_by_number(git_common_dir: &Path, number: u64) -> Option<PrInfo> {
    read_pr_info(git_common_dir)?
        .into_values()
        .find(|pr| pr.pr_number == number)
}

impl Graph {
    /// Builds a graph from `(branch, parent)` edges — the shape Graphite's
    /// db rows and wt's own tree records both reduce to, so either can build
    /// one without its own copy of the parent/child derivation below.
    pub fn from_edges<I>(edges: I) -> Graph
    where
        I: IntoIterator<Item = (String, Option<String>)>,
    {
        let edges: Vec<(String, Option<String>)> = edges.into_iter().collect();
        let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
        for (branch, parent) in &edges {
            nodes.entry(branch.clone()).or_default().parent = parent.clone();
        }
        // Only recorded into a parent that has its own node: a parent name
        // can dangle — a branch no tree or db row backs, such as trunk, or
        // one deleted out from under a stale reference.
        for (branch, parent) in &edges {
            if let Some(parent) = parent
                && nodes.contains_key(parent)
            {
                nodes.get_mut(parent).unwrap().children.push(branch.clone());
            }
        }
        Graph { nodes }
    }

    pub fn get(&self, branch: &str) -> Option<&Node> {
        self.nodes.get(branch)
    }

    pub fn get_mut(&mut self, branch: &str) -> Option<&mut Node> {
        self.nodes.get_mut(branch)
    }

    pub fn contains(&self, branch: &str) -> bool {
        self.nodes.contains_key(branch)
    }

    pub fn branch_names(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(String::as_str)
    }

    /// Branches with no tracked parent — either no parent at all, or a
    /// parent Graphite never tracked (trunk, or a branch `gt track` never
    /// ran on). Each is the entry point of one distinct stack.
    pub fn roots(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|(_, n)| {
                n.parent
                    .as_deref()
                    .is_none_or(|p| !self.nodes.contains_key(p))
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Ancestors of `branch`, trunk-first, not including `branch` itself.
    /// A cyclic parent chain — a malformed database, not a real stack —
    /// stops the walk instead of looping forever.
    pub fn downstack(&self, branch: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut current = self.nodes.get(branch).and_then(|n| n.parent.clone());
        while let Some(name) = current {
            if !seen.insert(name.clone()) {
                break;
            }
            chain.push(name.clone());
            current = self.nodes.get(&name).and_then(|n| n.parent.clone());
        }
        chain.reverse();
        chain
    }

    /// `branch` and everything above it, bottom-up (every branch appears
    /// after its parent). A cyclic `children` edge stops the walk instead
    /// of looping forever.
    pub fn upstack(&self, branch: &str) -> Vec<String> {
        if !self.nodes.contains_key(branch) {
            return Vec::new();
        }
        let mut order = vec![branch.to_string()];
        let mut seen: HashSet<String> = order.iter().cloned().collect();
        let mut frontier: VecDeque<String> = VecDeque::from([branch.to_string()]);
        while let Some(next) = frontier.pop_front() {
            let Some(node) = self.nodes.get(&next) else {
                continue;
            };
            for child in &node.children {
                if seen.insert(child.clone()) {
                    order.push(child.clone());
                    frontier.push_back(child.clone());
                }
            }
        }
        order
    }

    /// The whole stack `branch` belongs to: its ancestors, then `branch`,
    /// then everything above it — trunk-first, bottom-up throughout.
    pub fn stack(&self, branch: &str) -> Vec<String> {
        let mut result = self.downstack(branch);
        result.extend(self.upstack(branch));
        result
    }

    /// A bottom-up order over exactly `branches`: every branch comes after
    /// its parent when the parent is also in `branches`. Branches caught in
    /// a cycle within the subset are appended in a stable order at the end
    /// rather than left out or hung on forever.
    pub fn topo_order(&self, branches: &[String]) -> Vec<String> {
        let set: HashSet<&str> = branches.iter().map(String::as_str).collect();
        let mut indegree: BTreeMap<&str, usize> =
            branches.iter().map(|b| (b.as_str(), 0)).collect();
        let mut children_within: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for b in branches {
            if let Some(parent) = self.nodes.get(b.as_str()).and_then(|n| n.parent.as_deref())
                && set.contains(parent)
            {
                *indegree.get_mut(b.as_str()).unwrap() += 1;
                children_within.entry(parent).or_default().push(b.as_str());
            }
        }

        let mut ready: Vec<&str> = indegree
            .iter()
            .filter(|&(_, &d)| d == 0)
            .map(|(&b, _)| b)
            .collect();
        ready.sort_unstable();
        let mut queue: VecDeque<&str> = ready.into();

        let mut order = Vec::with_capacity(branches.len());
        let mut done: HashSet<&str> = HashSet::new();
        while let Some(b) = queue.pop_front() {
            if !done.insert(b) {
                continue;
            }
            order.push(b.to_string());
            if let Some(children) = children_within.get(b) {
                let mut next: Vec<&str> = Vec::new();
                for &c in children {
                    let d = indegree.get_mut(c).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        next.push(c);
                    }
                }
                next.sort_unstable();
                queue.extend(next);
            }
        }

        // Whatever is left sits in a cycle within this subset. Append it in
        // a stable order rather than dropping it or spinning forever.
        let mut leftover: Vec<&str> = branches
            .iter()
            .map(String::as_str)
            .filter(|b| !done.contains(b))
            .collect();
        leftover.sort_unstable();
        order.extend(leftover.into_iter().map(str::to_string));
        order
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use uuid::Uuid;

    fn temp_common_dir() -> PathBuf {
        std::env::temp_dir().join(format!("wt-graphite-test-{}", Uuid::now_v7()))
    }

    fn make_db(common_dir: &Path, rows: &[(&str, Option<&str>, Option<&str>)]) {
        fs::create_dir_all(common_dir).unwrap();
        let db = db_path(common_dir);
        let create = "CREATE TABLE branch_metadata (\
             branch_name TEXT PRIMARY KEY, parent_branch_name TEXT, \
             parent_branch_revision TEXT, last_submitted_version TEXT, state TEXT, \
             children TEXT, branch_revision TEXT, validation_result TEXT, \
             parent_head_revision TEXT);";
        run_sqlite(&db, create);
        for (name, parent, state) in rows {
            let parent_sql = parent.map_or("NULL".to_string(), |p| format!("'{p}'"));
            let state_sql = state.map_or("NULL".to_string(), |s| format!("'{s}'"));
            run_sqlite(
                &db,
                &format!(
                    "INSERT INTO branch_metadata (branch_name, parent_branch_name, state) \
                     VALUES ('{name}', {parent_sql}, {state_sql});"
                ),
            );
        }
    }

    fn run_sqlite(db: &Path, sql: &str) {
        let out = Command::new(SQLITE_BIN).arg(db).arg(sql).output().unwrap();
        assert!(
            out.status.success(),
            "sqlite3 {sql} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn available_is_false_without_a_database() {
        let dir = temp_common_dir();
        assert!(!available(&dir));
    }

    #[test]
    fn available_is_false_on_the_wrong_schema() {
        let dir = temp_common_dir();
        fs::create_dir_all(&dir).unwrap();
        run_sqlite(
            &db_path(&dir),
            "CREATE TABLE branch_metadata (branch_name TEXT);",
        );
        assert!(!available(&dir));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn graph_light_is_empty_not_an_error_when_branch_metadata_has_the_right_schema_but_no_rows() {
        let dir = temp_common_dir();
        make_db(&dir, &[]);
        assert!(available(&dir));
        let g = graph_light(&dir).unwrap();
        assert_eq!(g.branch_names().count(), 0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn available_is_true_with_the_expected_columns() {
        let dir = temp_common_dir();
        make_db(&dir, &[("master", None, Some("TRUNK"))]);
        assert!(available(&dir));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn graph_light_derives_children_from_parent_edges_not_the_children_column() {
        let dir = temp_common_dir();
        make_db(
            &dir,
            &[
                ("master", None, None),
                ("a", Some("master"), None),
                ("b", Some("a"), None),
            ],
        );
        let g = graph_light(&dir).unwrap();
        assert_eq!(g.get("master").unwrap().children, vec!["a".to_string()]);
        assert_eq!(g.get("a").unwrap().children, vec!["b".to_string()]);
        assert!(g.get("b").unwrap().children.is_empty());
        assert_eq!(g.get("a").unwrap().needs_restack, None);
        assert_eq!(g.get("a").unwrap().pr_number, None);
        fs::remove_dir_all(&dir).ok();
    }

    fn graph_of(edges: &[(&str, Option<&str>)]) -> Graph {
        Graph::from_edges(
            edges
                .iter()
                .map(|(name, parent)| ((*name).to_string(), parent.map(str::to_string))),
        )
    }

    #[test]
    fn downstack_is_trunk_first_and_excludes_the_branch() {
        let g = graph_of(&[("master", None), ("a", Some("master")), ("b", Some("a"))]);
        assert_eq!(
            g.downstack("b"),
            vec!["master".to_string(), "a".to_string()]
        );
        assert_eq!(g.downstack("master"), Vec::<String>::new());
    }

    #[test]
    fn upstack_includes_the_branch_and_is_bottom_up() {
        let g = graph_of(&[("master", None), ("a", Some("master")), ("b", Some("a"))]);
        assert_eq!(g.upstack("a"), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn stack_is_downstack_then_the_branch_then_upstack() {
        let g = graph_of(&[
            ("master", None),
            ("a", Some("master")),
            ("b", Some("a")),
            ("c", Some("a")),
        ]);
        let s = g.stack("a");
        assert_eq!(&s[..2], &["master".to_string(), "a".to_string()]);
        let tail: HashSet<&String> = s[2..].iter().collect();
        assert_eq!(tail, HashSet::from([&"b".to_string(), &"c".to_string()]));
    }

    #[test]
    fn roots_finds_branches_with_no_tracked_parent() {
        let g = graph_of(&[
            ("master", None),
            ("a", Some("master")),
            ("orphan", Some("deleted-branch")),
        ]);
        let mut roots = g.roots();
        roots.sort();
        assert_eq!(roots, vec!["master".to_string(), "orphan".to_string()]);
    }

    #[test]
    fn downstack_stops_on_a_cyclic_parent_chain() {
        let g = graph_of(&[("a", Some("b")), ("b", Some("a"))]);
        // Must return, not hang; the exact truncation point isn't load-bearing.
        let chain = g.downstack("a");
        assert!(chain.len() <= 2);
    }

    #[test]
    fn upstack_stops_on_a_cyclic_children_chain() {
        let g = graph_of(&[("a", Some("b")), ("b", Some("a"))]);
        let order = g.upstack("a");
        assert!(order.len() <= 2);
    }

    #[test]
    fn topo_order_puts_every_branch_after_its_parent() {
        let g = graph_of(&[
            ("master", None),
            ("a", Some("master")),
            ("b", Some("a")),
            ("c", Some("a")),
        ]);
        let branches = vec![
            "c".to_string(),
            "b".to_string(),
            "a".to_string(),
            "master".to_string(),
        ];
        let order = g.topo_order(&branches);
        let pos = |b: &str| order.iter().position(|x| x == b).unwrap();
        assert!(pos("master") < pos("a"));
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
    }

    #[test]
    fn topo_order_does_not_hang_on_a_cycle_and_keeps_every_branch() {
        let g = graph_of(&[("a", Some("b")), ("b", Some("a"))]);
        let branches = vec!["a".to_string(), "b".to_string()];
        let order = g.topo_order(&branches);
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["a".to_string(), "b".to_string()]);
    }

    fn write_pr_info(common_dir: &Path, prs: &[(&str, u64, &str)]) {
        fs::create_dir_all(common_dir).unwrap();
        let entries: Vec<String> = prs
            .iter()
            .map(|(branch, number, state)| {
                format!(
                    r#"{{"headRefName": "{branch}", "prNumber": {number},
                     "state": "{state}", "reviewDecision": null, "isDraft": false}}"#
                )
            })
            .collect();
        fs::write(
            common_dir.join(PR_INFO_FILE),
            format!(r#"{{"prInfos": [{}]}}"#, entries.join(",")),
        )
        .unwrap();
    }

    #[test]
    fn pr_branch_by_number_finds_the_matching_entry() {
        let dir = temp_common_dir();
        write_pr_info(
            &dir,
            &[("josh/a", 100, "OPEN"), ("josh/b", 18736, "MERGED")],
        );
        let pr = pr_branch_by_number(&dir, 18736).unwrap();
        assert_eq!(pr.head_ref_name, "josh/b");
        assert_eq!(pr.state, "MERGED");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pr_branch_by_number_is_none_when_no_pr_matches() {
        let dir = temp_common_dir();
        write_pr_info(&dir, &[("josh/a", 100, "OPEN")]);
        assert!(pr_branch_by_number(&dir, 18736).is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pr_branch_by_number_is_none_without_a_sidecar_file() {
        let dir = temp_common_dir();
        assert!(pr_branch_by_number(&dir, 18736).is_none());
    }
}
