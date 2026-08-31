use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::status_state_str;
use crate::store;

/// `Ok(None)` is the cancel path: escaping the picker ends a bare
/// `wt go` without launching anything, and that is not a failure.
pub fn pick_tree(store: &store::Store, cwd_repo: Option<&str>) -> Result<Option<Uuid>> {
    // A hot spare is unclaimed by definition, so it has nothing to open.
    let candidates: Vec<store::Tree> = store.trees.iter().filter(|t| !t.spare).cloned().collect();
    if candidates.is_empty() {
        bail!("no worktrees registered; create one first with `wt new <repo> --name \"...\"`");
    }

    let trees = ordered(&candidates, cwd_repo);
    let (header, lines) = build_lines(&trees);

    let fzf_bin = std::env::var("WT_FZF").unwrap_or_else(|_| "fzf".to_string());
    let preview_bin = std::env::current_exe()
        .context("locating this binary's path for the fzf preview command")?;
    let preview_cmd = format!(
        "{} __launch-preview {{1}}",
        shell_single_quote(&preview_bin.display().to_string())
    );

    let mut child = match Command::new(&fzf_bin)
        .arg("--delimiter=\t")
        // `{1}` and the line fzf prints both come from the raw input, not
        // from what --with-nth displays, so the uuid stays out of the
        // search text and still reaches the preview and the selection.
        .arg("--with-nth=2..")
        .arg("--layout=reverse")
        .arg("--no-multi")
        .arg("--ansi")
        .arg("--prompt=worktree> ")
        .arg(format!("--header={header}"))
        .arg(format!("--preview={preview_cmd}"))
        .arg("--preview-window=right,55%,border-left")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "a bare `wt go` opens a picker that needs fzf on PATH; `brew install fzf` \
             installs it, or pass a worktree name to skip the picker"
        ),
        Err(e) => return Err(e).context("spawning fzf"),
    };

    {
        let mut stdin = child.stdin.take().expect("fzf stdin was piped");
        // fzf can quit before reading every line, so a broken pipe here
        // means it already has an answer.
        for line in &lines {
            if writeln!(stdin, "{line}").is_err() {
                break;
            }
        }
    }

    let output = child.wait_with_output().context("waiting for fzf")?;
    selection_from(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
    )
}

/// The tree you want next is usually in the repo you are already sitting
/// in, or the one you just made, so those sort to the top.
fn ordered<'a>(trees: &'a [store::Tree], cwd_repo: Option<&str>) -> Vec<&'a store::Tree> {
    let mut sorted: Vec<&store::Tree> = trees.iter().collect();
    sorted.sort_by(|a, b| {
        let a_home = cwd_repo.is_some_and(|r| a.repo == r);
        let b_home = cwd_repo.is_some_and(|r| b.repo == r);
        b_home.cmp(&a_home).then_with(|| b.created.cmp(&a.created))
    });
    sorted
}

/// A compact stack position hint: the parent tree's name, when this tree
/// stacks on a branch another tree holds, or how many trees stack on top of
/// it, when it's the bottom of such a chain instead. Empty for a tree that
/// shares no stack edge with any other tree in `trees`.
fn stack_hint(t: &store::Tree, trees: &[&store::Tree]) -> String {
    if let Some(parent_branch) = &t.parent_branch
        && let Some(parent) = trees
            .iter()
            .find(|o| o.repo == t.repo && &o.branch == parent_branch)
    {
        return format!("^{}", parent.name);
    }
    let children = trees
        .iter()
        .filter(|o| o.repo == t.repo && o.parent_branch.as_deref() == Some(t.branch.as_str()))
        .count();
    if children > 0 {
        format!("+{children}")
    } else {
        String::new()
    }
}

fn build_lines(trees: &[&store::Tree]) -> (String, Vec<String>) {
    let states: Vec<String> = trees.iter().map(|t| status_state_str(t)).collect();
    let hints: Vec<String> = trees.iter().map(|t| stack_hint(t, trees)).collect();
    // A column that would be blank for every row is noise; solo-tree
    // listings keep exactly the layout they always had.
    let show_stack = hints.iter().any(|h| !h.is_empty());

    let w = |header: &str, vals: &mut dyn Iterator<Item = usize>| {
        vals.chain(std::iter::once(header.len())).max().unwrap_or(0)
    };
    let name_w = w("NAME", &mut trees.iter().map(|t| t.name.chars().count()));
    let repo_w = w("REPO", &mut trees.iter().map(|t| t.repo.chars().count()));
    let branch_w = w(
        "BRANCH",
        &mut trees.iter().map(|t| t.branch.chars().count()),
    );
    let state_w = w("STATE", &mut states.iter().map(String::len));
    let stack_w = w("STACK", &mut hints.iter().map(String::len));

    let header = if show_stack {
        format!(
            "{:<name_w$} {:<repo_w$} {:<branch_w$} {:<state_w$} {:<stack_w$}",
            "NAME", "REPO", "BRANCH", "STATE", "STACK"
        )
    } else {
        format!(
            "{:<name_w$} {:<repo_w$} {:<branch_w$} {:<state_w$}",
            "NAME", "REPO", "BRANCH", "STATE"
        )
    }
    .trim_end()
    .to_string();

    let lines = trees
        .iter()
        .zip(&states)
        .zip(&hints)
        .map(|((t, state), hint)| {
            let display = if show_stack {
                format!(
                    "{:<name_w$} {:<repo_w$} {:<branch_w$} {:<state_w$} {:<stack_w$}",
                    t.name, t.repo, t.branch, state, hint
                )
            } else {
                format!(
                    "{:<name_w$} {:<repo_w$} {:<branch_w$} {:<state_w$}",
                    t.name, t.repo, t.branch, state
                )
            };
            format!("{}\t{}", t.id, display.trim_end())
        })
        .collect();

    (header, lines)
}

fn selection_from(code: Option<i32>, stdout: &str) -> Result<Option<Uuid>> {
    match code {
        Some(0) => {
            let line = stdout.lines().next().unwrap_or("");
            let id = line.split('\t').next().unwrap_or("");
            Uuid::parse_str(id)
                .with_context(|| format!("fzf printed a line with no valid uuid: {line:?}"))
                .map(Some)
        }
        Some(1) | Some(130) => Ok(None),
        Some(other) => bail!("fzf exited with status {other}"),
        None => bail!("fzf was terminated by a signal"),
    }
}

/// fzf runs `--preview` through `sh -c`, so the current binary's path needs
/// shell quoting, not just argv quoting.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use std::path::PathBuf;
    use store::{Tree, TreeState};

    fn tree_at(repo: &str, name: &str, branch: &str, age_secs: i64) -> Tree {
        Tree {
            id: Uuid::now_v7(),
            repo: repo.into(),
            name: name.into(),
            branch: branch.into(),
            path: PathBuf::from(format!("/tmp/{name}")),
            created: Utc::now() - Duration::seconds(age_secs),
            state: TreeState::Ready,
            step_label: None,
            step_index: None,
            step_total: None,
            log_path: None,
            provision_pid: None,
            parent_branch: None,
            parent_revision: None,
            pending_restack: false,
            pr_number: None,
            spare: false,
        }
    }

    #[test]
    fn ordered_puts_cwd_repo_first_then_newest_within_each_group() {
        let home_old = tree_at("home", "home-old", "b1", 100);
        let home_new = tree_at("home", "home-new", "b2", 10);
        let other_old = tree_at("other", "other-old", "b3", 90);
        let other_new = tree_at("other", "other-new", "b4", 5);
        let trees = vec![
            home_old.clone(),
            other_new.clone(),
            home_new.clone(),
            other_old.clone(),
        ];

        let names: Vec<&str> = ordered(&trees, Some("home"))
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["home-new", "home-old", "other-new", "other-old"]
        );
    }

    #[test]
    fn ordered_with_no_cwd_repo_is_newest_first() {
        let oldest = tree_at("a", "oldest", "b1", 300);
        let middle = tree_at("b", "middle", "b2", 200);
        let newest = tree_at("c", "newest", "b3", 10);
        let trees = vec![oldest.clone(), newest.clone(), middle.clone()];

        let names: Vec<&str> = ordered(&trees, None)
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["newest", "middle", "oldest"]);
    }

    #[test]
    fn build_lines_pads_columns_to_the_widest_row_or_header() {
        let a = tree_at("mono", "a", "ba", 10);
        let b = tree_at("mono", "longname", "josh/longname", 5);
        let refs = vec![&a, &b];

        let (header, lines) = build_lines(&refs);

        let expected_header = format!(
            "{:<8} {:<4} {:<13} {:<5}",
            "NAME", "REPO", "BRANCH", "STATE"
        );
        assert_eq!(header, expected_header.trim_end());

        let expected_a = format!("{:<8} {:<4} {:<13} {:<5}", "a", "mono", "ba", "ready");
        assert_eq!(lines[0], format!("{}\t{}", a.id, expected_a.trim_end()));

        let expected_b = format!(
            "{:<8} {:<4} {:<13} {:<5}",
            "longname", "mono", "josh/longname", "ready"
        );
        assert_eq!(lines[1], format!("{}\t{}", b.id, expected_b.trim_end()));
    }

    #[test]
    fn build_lines_puts_the_uuid_in_the_first_tab_field() {
        let a = tree_at("mono", "fix login", "josh/fix-login", 1);
        let refs = vec![&a];
        let (_, lines) = build_lines(&refs);

        let mut fields = lines[0].splitn(2, '\t');
        assert_eq!(fields.next(), Some(a.id.to_string().as_str()));
        assert!(fields.next().unwrap().starts_with("fix login"));
    }

    #[test]
    fn build_lines_hints_at_stack_position_only_for_trees_that_have_one() {
        let root = tree_at("mono", "root pr", "a", 10);
        let mut child = tree_at("mono", "child pr", "b", 5);
        child.parent_branch = Some("a".to_string());
        let solo = tree_at("mono", "solo", "c", 1);
        let refs = vec![&root, &child, &solo];

        let (header, lines) = build_lines(&refs);
        assert!(header.contains("STACK"), "header was: {header}");

        assert!(lines[0].ends_with("+1"), "root line was: {}", lines[0]);
        assert!(
            lines[1].ends_with("^root pr"),
            "child line was: {}",
            lines[1]
        );
        assert!(
            !lines[2].contains('^') && !lines[2].contains('+'),
            "solo tree must carry no stack hint: {}",
            lines[2]
        );
    }

    #[test]
    fn build_lines_omits_the_stack_column_when_nothing_is_stacked() {
        let a = tree_at("mono", "a", "josh/a", 10);
        let b = tree_at("mono", "b", "josh/b", 5);
        let refs = vec![&a, &b];

        let (header, lines) = build_lines(&refs);
        assert!(!header.contains("STACK"), "header was: {header}");
        for line in &lines {
            assert!(!line.contains("  +") && !line.contains("  ^"), "{line}");
        }
    }

    #[test]
    fn selection_from_reads_the_uuid_before_the_first_tab() {
        let id = Uuid::now_v7();
        let stdout = format!("{id}\tfix login  mono  josh/fix-login  ready\n");
        assert_eq!(selection_from(Some(0), &stdout).unwrap(), Some(id));
    }

    #[test]
    fn selection_from_treats_no_match_and_cancel_as_none() {
        assert_eq!(selection_from(Some(1), "").unwrap(), None);
        assert_eq!(selection_from(Some(130), "").unwrap(), None);
    }

    #[test]
    fn selection_from_errors_on_an_unexpected_exit_code() {
        let err = selection_from(Some(2), "").unwrap_err();
        assert!(err.to_string().contains('2'), "message was: {err}");
    }

    #[test]
    fn selection_from_errors_on_a_malformed_line() {
        let err = selection_from(Some(0), "not-a-uuid\tsomething\n").unwrap_err();
        assert!(
            err.to_string().contains("no valid uuid"),
            "message was: {err}"
        );
    }
}
