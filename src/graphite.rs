//! Read-only access to Graphite's stack graph: a private SQLite file plus a
//! JSON sidecar, both in the git common dir. `wt` only ever reads them —
//! every mutation goes through `gt`. Any missing or unexpected piece (the
//! `sqlite3` binary, either file, a schema change) degrades to "no stack
//! info" rather than an error.

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
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Debug, Deserialize)]
struct BranchRow {
    branch_name: String,
    parent_branch_name: Option<String>,
    parent_branch_revision: Option<String>,
    state: Option<String>,
}

/// Graphite's GitHub mirror for one branch's pull request, read from
/// `.graphite_pr_info`. `state` is `OPEN`, `MERGED`, or `CLOSED`.
#[derive(Debug, Clone, Deserialize)]
struct PrInfo {
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "prNumber")]
    pr_number: u64,
    state: String,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    #[serde(rename = "isDraft")]
    is_draft: bool,
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
    pub state: Option<String>,
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

/// One query for the whole graph — cheap enough to run on every `wt`
/// invocation that wants it, and `-readonly` so it never contends with a
/// concurrent `gt` write or leaves `-wal`/`-shm` files behind in the shared
/// git dir.
pub fn graph(git_common_dir: &Path) -> Result<Graph> {
    let json = query_json(
        git_common_dir,
        "SELECT branch_name, parent_branch_name, parent_branch_revision, state \
         FROM branch_metadata",
    )?;
    let rows: Vec<BranchRow> = serde_json::from_str(&json).context("parsing branch_metadata")?;

    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    for row in &rows {
        let node = nodes.entry(row.branch_name.clone()).or_default();
        node.parent = row.parent_branch_name.clone();
        node.state = row.state.clone();
    }
    // Only recorded into a parent that has its own row: `parent_branch_name`
    // can dangle (a deleted branch Graphite never dropped the reference to).
    for row in &rows {
        if let Some(parent) = &row.parent_branch_name
            && nodes.contains_key(parent)
        {
            nodes
                .get_mut(parent)
                .unwrap()
                .children
                .push(row.branch_name.clone());
        }
    }

    let pr_infos = read_pr_info(git_common_dir);
    if let Some(prs) = &pr_infos {
        for pr in prs.values() {
            if let Some(node) = nodes.get_mut(&pr.head_ref_name) {
                node.pr_number = Some(pr.pr_number);
                node.pr_state = Some(pr.state.clone());
                node.pr_review_decision = pr.review_decision.clone();
                node.pr_draft = Some(pr.is_draft);
            }
        }
    }

    if let Ok(heads) = live_heads(git_common_dir) {
        for row in &rows {
            let needs = needs_restack(git_common_dir, row, &heads, pr_infos.as_ref());
            if let Some(node) = nodes.get_mut(&row.branch_name) {
                node.needs_restack = needs;
            }
        }
    }

    Ok(Graph { nodes })
}

/// A branch needs a restack when its parent's current head is not an
/// ancestor of it, unless the branch's own pull request is already merged
/// or closed — Graphite's `(merged)` annotation masks `(needs restack)`, so
/// a merged branch is never reported as needing one regardless of its
/// actual git shape.
fn needs_restack(
    git_common_dir: &Path,
    row: &BranchRow,
    heads: &HashMap<String, String>,
    pr_infos: Option<&HashMap<String, PrInfo>>,
) -> Option<bool> {
    let parent = row.parent_branch_name.as_deref()?;
    let parent_head = heads.get(parent)?;
    let branch_head = heads.get(&row.branch_name)?;

    // If the parent hasn't moved since this branch last recorded its fork
    // point, that fork point is trivially still on the parent's line, and
    // therefore still an ancestor of this branch too — no need to pay for
    // `merge-base` on the ~80% of branches this is true for.
    let is_ancestor = if row.parent_branch_revision.as_deref() == Some(parent_head.as_str()) {
        true
    } else {
        is_ancestor(git_common_dir, parent_head, branch_head).ok()?
    };

    if is_ancestor {
        return Some(false);
    }
    let masked = pr_infos?
        .get(&row.branch_name)
        .is_some_and(|pr| matches!(pr.state.as_str(), "MERGED" | "CLOSED"));
    Some(!masked)
}

fn read_pr_info(git_common_dir: &Path) -> Option<HashMap<String, PrInfo>> {
    let bytes = std::fs::read(git_common_dir.join(PR_INFO_FILE)).ok()?;
    let file: PrInfoFile = serde_json::from_slice(&bytes).ok()?;
    Some(
        file.pr_infos
            .into_iter()
            .map(|pr| (pr.head_ref_name.clone(), pr))
            .collect(),
    )
}

fn git_common_dir_cmd(git_common_dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("--git-dir")
        .arg(git_common_dir)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))
}

/// Every local branch's current tip, in one process call. Works from any
/// path, including one with no worktree checked out, since `--git-dir`
/// bypasses git's usual repository discovery.
fn live_heads(git_common_dir: &Path) -> Result<HashMap<String, String>> {
    let out = git_common_dir_cmd(
        git_common_dir,
        &[
            "for-each-ref",
            "--format=%(refname:short) %(objectname)",
            "refs/heads/",
        ],
    )?;
    if !out.status.success() {
        bail!(
            "git for-each-ref failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut heads = HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some((name, sha)) = line.split_once(' ') {
            heads.insert(name.to_string(), sha.to_string());
        }
    }
    Ok(heads)
}

fn is_ancestor(git_common_dir: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let out = git_common_dir_cmd(
        git_common_dir,
        &["merge-base", "--is-ancestor", ancestor, descendant],
    )?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "git merge-base --is-ancestor {ancestor} {descendant} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

impl Graph {
    pub fn get(&self, branch: &str) -> Option<&Node> {
        self.nodes.get(branch)
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
    fn available_is_true_with_the_expected_columns() {
        let dir = temp_common_dir();
        make_db(&dir, &[("master", None, Some("TRUNK"))]);
        assert!(available(&dir));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn graph_derives_children_from_parent_edges_not_the_children_column() {
        let dir = temp_common_dir();
        make_db(
            &dir,
            &[
                ("master", None, None),
                ("a", Some("master"), None),
                ("b", Some("a"), None),
            ],
        );
        let g = graph(&dir).unwrap();
        assert_eq!(g.get("master").unwrap().children, vec!["a".to_string()]);
        assert_eq!(g.get("a").unwrap().children, vec!["b".to_string()]);
        assert!(g.get("b").unwrap().children.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    fn graph_of(edges: &[(&str, Option<&str>)]) -> Graph {
        let mut nodes = BTreeMap::new();
        for (name, parent) in edges {
            nodes
                .entry((*name).to_string())
                .or_insert_with(Node::default)
                .parent = parent.map(str::to_string);
        }
        for (name, parent) in edges {
            if let Some(p) = parent
                && nodes.contains_key(*p)
            {
                nodes
                    .get_mut(*p)
                    .unwrap()
                    .children
                    .push((*name).to_string());
            }
        }
        Graph { nodes }
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

    fn run_git(args: &[&str], cwd: &Path) {
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

    /// A real repo with `master` and `child` (parent `master`), where
    /// `master` has since moved past the commit `child` forked from —
    /// the shape that requires an actual `merge-base` check, not just the
    /// recorded-revision prefilter.
    fn fixture_with_stale_child() -> (PathBuf, String) {
        let dir = temp_common_dir();
        fs::create_dir_all(&dir).unwrap();
        run_git(&["init", "-q", "-b", "master"], &dir);
        run_git(&["config", "user.email", "t@t"], &dir);
        run_git(&["config", "user.name", "t"], &dir);
        fs::write(dir.join("f.txt"), "0\n").unwrap();
        run_git(&["add", "-A"], &dir);
        run_git(&["commit", "-qm", "c0"], &dir);
        let fork_point = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&dir)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        run_git(&["checkout", "-qb", "child"], &dir);
        fs::write(dir.join("child.txt"), "1\n").unwrap();
        run_git(&["add", "-A"], &dir);
        run_git(&["commit", "-qm", "c1"], &dir);

        run_git(&["checkout", "-q", "master"], &dir);
        fs::write(dir.join("f.txt"), "1\n").unwrap();
        run_git(&["add", "-A"], &dir);
        run_git(&["commit", "-qm", "c2"], &dir);

        let common_dir = dir.join(".git");
        make_db(
            &common_dir,
            &[
                ("master", None, Some("TRUNK")),
                ("child", Some("master"), None),
            ],
        );
        run_sqlite(
            &db_path(&common_dir),
            &format!(
                "UPDATE branch_metadata SET parent_branch_revision = '{fork_point}' \
                 WHERE branch_name = 'child';"
            ),
        );
        (common_dir, fork_point)
    }

    fn write_pr_info(common_dir: &Path, branch: &str, state: &str) {
        fs::write(
            common_dir.join(PR_INFO_FILE),
            format!(
                r#"{{"prInfos": [{{"headRefName": "{branch}", "prNumber": 1,
                 "state": "{state}", "reviewDecision": null, "isDraft": false}}]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn needs_restack_is_true_when_parent_moved_past_the_fork_point_and_the_pr_is_open() {
        let (common_dir, _) = fixture_with_stale_child();
        write_pr_info(&common_dir, "child", "OPEN");
        let g = graph(&common_dir).unwrap();
        assert_eq!(g.get("child").unwrap().needs_restack, Some(true));
        assert_eq!(g.get("master").unwrap().needs_restack, None);
        fs::remove_dir_all(common_dir.parent().unwrap()).ok();
    }

    #[test]
    fn needs_restack_is_masked_by_a_merged_pr() {
        let (common_dir, _) = fixture_with_stale_child();
        write_pr_info(&common_dir, "child", "MERGED");
        let g = graph(&common_dir).unwrap();
        assert_eq!(g.get("child").unwrap().needs_restack, Some(false));
        assert_eq!(g.get("child").unwrap().pr_state.as_deref(), Some("MERGED"));
        fs::remove_dir_all(common_dir.parent().unwrap()).ok();
    }

    #[test]
    fn needs_restack_is_unknown_without_readable_pr_info() {
        let (common_dir, _) = fixture_with_stale_child();
        // No `.graphite_pr_info` written — masking can't be ruled out.
        let g = graph(&common_dir).unwrap();
        assert_eq!(g.get("child").unwrap().needs_restack, None);
        fs::remove_dir_all(common_dir.parent().unwrap()).ok();
    }

    #[test]
    fn needs_restack_is_false_when_the_recorded_fork_point_is_still_current() {
        // `child` freshly branched off `master`'s current tip: the prefilter
        // alone settles this without a `merge-base` call.
        let dir = temp_common_dir();
        fs::create_dir_all(&dir).unwrap();
        run_git(&["init", "-q", "-b", "master"], &dir);
        run_git(&["config", "user.email", "t@t"], &dir);
        run_git(&["config", "user.name", "t"], &dir);
        fs::write(dir.join("f.txt"), "0\n").unwrap();
        run_git(&["add", "-A"], &dir);
        run_git(&["commit", "-qm", "c0"], &dir);
        let head = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&dir)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        run_git(&["checkout", "-qb", "child"], &dir);
        fs::write(dir.join("child.txt"), "1\n").unwrap();
        run_git(&["add", "-A"], &dir);
        run_git(&["commit", "-qm", "c1"], &dir);

        let common_dir = dir.join(".git");
        make_db(
            &common_dir,
            &[
                ("master", None, Some("TRUNK")),
                ("child", Some("master"), None),
            ],
        );
        run_sqlite(
            &db_path(&common_dir),
            &format!(
                "UPDATE branch_metadata SET parent_branch_revision = '{head}' \
                 WHERE branch_name = 'child';"
            ),
        );
        let g = graph(&common_dir).unwrap();
        assert_eq!(g.get("child").unwrap().needs_restack, Some(false));
        fs::remove_dir_all(dir).ok();
    }
}
