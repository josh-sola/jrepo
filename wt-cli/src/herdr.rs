//! Places a launched agent in a herdr workspace instead of execing it in the
//! calling terminal. See `wt-cli/README.md`'s herdr section for the feature
//! and `herdr <sub> -h` / `herdr api schema --json` for the CLI this shells
//! out to.

use std::env;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::agent::Agent;
use crate::config::Config;

const CODE_TAB_LABEL: &str = "code";

/// `--here` also stops a placed run from placing again, since the placed
/// command always includes it.
pub fn active(config: &Config, here: bool) -> bool {
    !here && config.features.herdr.is_some() && env::var_os("HERDR_ENV").is_some()
}

fn herdr_bin() -> String {
    env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

/// Runs one `herdr` CLI call and returns its `.result`.
fn run_herdr(args: &[String]) -> Result<Value> {
    let bin = herdr_bin();
    let output = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow!("`{bin}` is not on PATH; herdr placement needs the herdr CLI installed")
            }
            _ => anyhow!("could not run `{bin} {}`: {e}", args.join(" ")),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let text = herdr_error_text(&stderr);
        if text.is_empty() {
            bail!("`{bin} {}` failed: {}", args.join(" "), output.status);
        }
        bail!("`{bin} {}` failed: {text}", args.join(" "));
    }

    // `pane run` succeeds with no output at all; only calls whose result is
    // read later need a JSON body.
    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parsing `{bin} {}` output as JSON", args.join(" ")))?;
    value
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("`{bin} {}` printed no 'result' field", args.join(" ")))
}

/// herdr's CLI errors are JSON on stderr and are decoded to `code: message`;
/// anything else is kept as plain text.
fn herdr_error_text(stderr: &str) -> String {
    let trimmed = stderr.trim();
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .and_then(|v| {
            let error = v.get("error")?;
            let code = error.get("code")?.as_str()?;
            let message = error.get("message")?.as_str()?;
            Some(format!("{code}: {message}"))
        })
        .unwrap_or_else(|| trimmed.to_string())
}

/// `scratch` selects the workspace-by-label lookup a `@label` launch needs,
/// since it has no tree path to match against.
pub fn place(
    cwd: &Path,
    base: &Path,
    repo: &str,
    label: &str,
    scratch: bool,
    inner_argv: &[String],
) -> Result<()> {
    let cwd = cwd.to_string_lossy().to_string();
    let pane_id = if scratch {
        place_scratch(&cwd, label)?
    } else {
        place_tree(&cwd, &base.to_string_lossy(), repo, label)?
    };
    run_herdr(&pane_rename_argv(&pane_id, label))?;
    run_herdr(&pane_run_argv(&pane_id, &shell_join(inner_argv)))?;
    Ok(())
}

/// herdr rejects `worktree open` unless it is issued from the repo's parent
/// workspace (`linked_worktree_source`), so that workspace is found or
/// created first; a rejected open still falls back to a plain workspace so
/// the user is never left with nothing.
fn place_tree(cwd: &str, base: &str, repo: &str, label: &str) -> Result<String> {
    let list = run_herdr(&worktree_list_argv(cwd))?;
    let worktrees = array_field(&list, "worktrees")?;
    if let Some(workspace_id) = workspace_for_tree_path(worktrees, cwd) {
        return pane_id_from(&run_herdr(&tab_create_argv(&workspace_id, cwd, label))?);
    }

    let workspace_list = run_herdr(&workspace_list_argv())?;
    let workspaces = array_field(&workspace_list, "workspaces")?;
    if let Some(workspace_id) = workspace_for_checkout_path(workspaces, cwd)
        .or_else(|| workspace_for_label(workspaces, label))
    {
        return pane_id_from(&run_herdr(&tab_create_argv(&workspace_id, cwd, label))?);
    }

    let parent_workspace_id = match parent_workspace_id(worktrees) {
        Some(id) => id,
        None => {
            let parent = parent_path(worktrees).unwrap_or_else(|| base.to_string());
            let created = run_herdr(&parent_workspace_create_argv(&parent, repo))?;
            workspace_id_from(&created)?
        }
    };

    match run_herdr(&worktree_open_argv(&parent_workspace_id, cwd, label)) {
        Ok(result) => code_tab_then_agent_pane(&result, cwd, label),
        Err(e) => {
            eprintln!("herdr worktree open failed ({e}); opening a plain workspace instead");
            let created = run_herdr(&workspace_create_argv(cwd, label))?;
            code_tab_then_agent_pane(&created, cwd, label)
        }
    }
}

/// A newly created tree workspace opens focused on the agent, so its root
/// pane becomes the editor tab and the agent gets the next tab.
fn code_tab_then_agent_pane(created: &Value, cwd: &str, label: &str) -> Result<String> {
    let tab_id = tab_id_from(created)?;
    let root_pane_id = pane_id_from(created)?;
    let workspace_id = workspace_id_from(created)?;
    run_herdr(&tab_rename_argv(&tab_id, CODE_TAB_LABEL))?;
    run_herdr(&pane_rename_argv(&root_pane_id, CODE_TAB_LABEL))?;
    run_herdr(&pane_run_argv(
        &root_pane_id,
        &shell_join(&code_tab_command(cwd, label)),
    ))?;
    pane_id_from(&run_herdr(&tab_create_argv(&workspace_id, cwd, label))?)
}

/// `emacsclient -t` alone opens in the daemon's own directory, so the frame
/// is set up around the tree instead. The `do-` treemacs entry point is used
/// because the interactive one prompts on a name collision, which would hang
/// an `--eval`; the window is reselected because treemacs steals focus.
fn code_tab_command(cwd: &str, label: &str) -> Vec<String> {
    let cwd = elisp_string(cwd);
    let label = elisp_string(label);
    vec![
        "emacsclient".to_string(),
        "-t".to_string(),
        "--eval".to_string(),
        format!(
            "(progn (require (quote treemacs)) (let ((w (selected-window))) \
             (with-current-buffer (window-buffer w) (cd {cwd})) \
             (treemacs-do-add-project-to-workspace {cwd} {label}) \
             (treemacs-select-window) (select-window w)))"
        ),
    ]
}

fn elisp_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn place_scratch(cwd: &str, label: &str) -> Result<String> {
    let list = run_herdr(&workspace_list_argv())?;
    let workspaces = array_field(&list, "workspaces")?;
    match workspace_for_label(workspaces, label) {
        Some(workspace_id) => {
            pane_id_from(&run_herdr(&tab_create_argv(&workspace_id, cwd, label))?)
        }
        None => pane_id_from(&run_herdr(&workspace_create_argv(cwd, label))?),
    }
}

fn array_field<'a>(result: &'a Value, field: &str) -> Result<&'a [Value]> {
    result
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow!("herdr response missing array field '{field}': {result}"))
}

fn pane_id_from(result: &Value) -> Result<String> {
    result
        .get("root_pane")
        .and_then(|p| p.get("pane_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("herdr response missing .root_pane.pane_id: {result}"))
}

fn workspace_id_from(result: &Value) -> Result<String> {
    result
        .get("workspace")
        .and_then(|w| w.get("workspace_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("herdr response missing .workspace.workspace_id: {result}"))
}

fn tab_id_from(result: &Value) -> Result<String> {
    result
        .get("tab")
        .and_then(|t| t.get("tab_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("herdr response missing .tab.tab_id: {result}"))
}

/// The parent checkout is the entry with `is_linked_worktree: false`; a
/// null id means one must be created.
fn parent_workspace_id(worktrees: &[Value]) -> Option<String> {
    worktrees.iter().find_map(|w| {
        if w.get("is_linked_worktree").and_then(Value::as_bool) != Some(false) {
            return None;
        }
        w.get("open_workspace_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn parent_path(worktrees: &[Value]) -> Option<String> {
    worktrees.iter().find_map(|w| {
        if w.get("is_linked_worktree").and_then(Value::as_bool) != Some(false) {
            return None;
        }
        w.get("path").and_then(Value::as_str).map(str::to_string)
    })
}

/// `herdr worktree list`'s `worktrees[].path` matched exactly against the
/// canonical tree path decides reuse vs a new workspace; `open_workspace_id`
/// is null both when nothing matched and when the worktree exists but has no
/// open workspace, and both mean the same thing here: open one.
fn workspace_for_tree_path(worktrees: &[Value], canonical_path: &str) -> Option<String> {
    worktrees.iter().find_map(|w| {
        if w.get("path").and_then(Value::as_str) != Some(canonical_path) {
            return None;
        }
        w.get("open_workspace_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

/// A workspace made by the fallback never appears in `worktree list`, so it
/// is matched by `.worktree.checkout_path` instead.
fn workspace_for_checkout_path(workspaces: &[Value], canonical_path: &str) -> Option<String> {
    workspaces.iter().find_map(|w| {
        let checkout_path = w
            .get("worktree")
            .and_then(|wt| wt.get("checkout_path"))
            .and_then(Value::as_str);
        if checkout_path != Some(canonical_path) {
            return None;
        }
        w.get("workspace_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

/// Scratch sessions have no tree path to match, so the workspace `herdr
/// workspace list` reports with a matching `label` is the reuse signal
/// instead.
fn workspace_for_label(workspaces: &[Value], label: &str) -> Option<String> {
    workspaces.iter().find_map(|w| {
        if w.get("label").and_then(Value::as_str) != Some(label) {
            return None;
        }
        w.get("workspace_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

/// `--cwd` tells herdr which repo's worktrees to list; the plugin pane's own
/// cwd is the plugin directory, not the tree, so without it a tree in a
/// different repo would never match and placement would open a duplicate
/// workspace.
fn worktree_list_argv(cwd: &str) -> Vec<String> {
    vec![
        "worktree".to_string(),
        "list".to_string(),
        "--cwd".to_string(),
        cwd.to_string(),
    ]
}

fn workspace_list_argv() -> Vec<String> {
    vec!["workspace".to_string(), "list".to_string()]
}

fn worktree_open_argv(parent_workspace_id: &str, path: &str, label: &str) -> Vec<String> {
    vec![
        "worktree".to_string(),
        "open".to_string(),
        "--workspace".to_string(),
        parent_workspace_id.to_string(),
        "--path".to_string(),
        path.to_string(),
        "--label".to_string(),
        label.to_string(),
        "--focus".to_string(),
    ]
}

fn workspace_create_argv(cwd: &str, label: &str) -> Vec<String> {
    vec![
        "workspace".to_string(),
        "create".to_string(),
        "--cwd".to_string(),
        cwd.to_string(),
        "--label".to_string(),
        label.to_string(),
        "--focus".to_string(),
    ]
}

/// `--no-focus` because the parent workspace exists only so the open has
/// somewhere to attach.
fn parent_workspace_create_argv(cwd: &str, repo: &str) -> Vec<String> {
    vec![
        "workspace".to_string(),
        "create".to_string(),
        "--cwd".to_string(),
        cwd.to_string(),
        "--label".to_string(),
        repo.to_string(),
        "--no-focus".to_string(),
    ]
}

fn tab_create_argv(workspace_id: &str, cwd: &str, label: &str) -> Vec<String> {
    vec![
        "tab".to_string(),
        "create".to_string(),
        "--workspace".to_string(),
        workspace_id.to_string(),
        "--cwd".to_string(),
        cwd.to_string(),
        "--label".to_string(),
        label.to_string(),
        "--focus".to_string(),
    ]
}

fn pane_rename_argv(pane_id: &str, label: &str) -> Vec<String> {
    vec![
        "pane".to_string(),
        "rename".to_string(),
        pane_id.to_string(),
        label.to_string(),
    ]
}

fn tab_rename_argv(tab_id: &str, label: &str) -> Vec<String> {
    vec![
        "tab".to_string(),
        "rename".to_string(),
        tab_id.to_string(),
        label.to_string(),
    ]
}

fn pane_run_argv(pane_id: &str, command: &str) -> Vec<String> {
    vec![
        "pane".to_string(),
        "run".to_string(),
        pane_id.to_string(),
        command.to_string(),
    ]
}

/// The inner `wt go` command a placed pane runs. `--here` stops it from
/// placing again; the selector is the tree's uuid, or a scratch launch's
/// `@label` (which needs `--repo` since it has no tree to infer one from).
pub fn inner_argv(
    exe: &str,
    selector: &str,
    repo_for_scratch: Option<&str>,
    agent: Agent,
    profile: Option<&[String]>,
    args: &[String],
) -> Vec<String> {
    let mut argv = vec![exe.to_string(), "go".to_string(), selector.to_string()];
    if let Some(repo) = repo_for_scratch {
        argv.push("--repo".to_string());
        argv.push(repo.to_string());
    }
    argv.push("--here".to_string());
    argv.push(format!("--{}", agent.executable()));
    if let Some(profile) = profile
        && !profile.is_empty()
    {
        argv.push("--profile".to_string());
        argv.push(profile.join(","));
    }
    if !args.is_empty() {
        argv.push("--".to_string());
        argv.extend(args.iter().cloned());
    }
    argv
}

/// `herdr pane run` sends its `COMMAND...` argv to the pane's shell as one
/// joined line (there is no argv-preserving exec), so each token is quoted
/// here rather than left to the CLI.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn shell_join<S: AsRef<str>>(argv: &[S]) -> String {
    argv.iter()
        .map(|a| shell_single_quote(a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workspace_for_tree_path_matches_exact_path() {
        let worktrees = vec![
            json!({"path": "/repos/a", "open_workspace_id": "w1"}),
            json!({"path": "/repos/b", "open_workspace_id": null}),
        ];
        assert_eq!(
            workspace_for_tree_path(&worktrees, "/repos/a"),
            Some("w1".to_string())
        );
    }

    #[test]
    fn workspace_for_tree_path_is_none_for_a_null_or_missing_workspace() {
        let worktrees = vec![json!({"path": "/repos/b", "open_workspace_id": null})];
        assert_eq!(workspace_for_tree_path(&worktrees, "/repos/b"), None);
        assert_eq!(workspace_for_tree_path(&worktrees, "/repos/nope"), None);
    }

    #[test]
    fn workspace_for_label_matches_exact_label() {
        let workspaces = vec![
            json!({"workspace_id": "w1", "label": "@scratch"}),
            json!({"workspace_id": "w2", "label": "other"}),
        ];
        assert_eq!(
            workspace_for_label(&workspaces, "@scratch"),
            Some("w1".to_string())
        );
        assert_eq!(workspace_for_label(&workspaces, "nope"), None);
    }

    #[test]
    fn pane_id_from_reads_the_nested_field() {
        let result = json!({"root_pane": {"pane_id": "w1:p1"}});
        assert_eq!(pane_id_from(&result).unwrap(), "w1:p1");
    }

    #[test]
    fn pane_id_from_errors_when_missing() {
        let result = json!({"root_pane": {}});
        assert!(pane_id_from(&result).is_err());
    }

    #[test]
    fn worktree_list_argv_passes_cwd() {
        assert_eq!(
            worktree_list_argv("/repos/a"),
            vec!["worktree", "list", "--cwd", "/repos/a"]
        );
    }

    #[test]
    fn worktree_open_argv_matches_the_cli() {
        assert_eq!(
            worktree_open_argv("w9", "/repos/a", "fix login"),
            vec![
                "worktree",
                "open",
                "--workspace",
                "w9",
                "--path",
                "/repos/a",
                "--label",
                "fix login",
                "--focus"
            ]
        );
    }

    #[test]
    fn parent_workspace_create_argv_matches_the_cli() {
        assert_eq!(
            parent_workspace_create_argv("/repos/base", "myrepo"),
            vec![
                "workspace",
                "create",
                "--cwd",
                "/repos/base",
                "--label",
                "myrepo",
                "--no-focus"
            ]
        );
    }

    #[test]
    fn parent_workspace_id_matches_the_non_linked_entry() {
        let worktrees = vec![
            json!({"path": "/repos/a", "is_linked_worktree": true, "open_workspace_id": "w1"}),
            json!({"path": "/repos/base", "is_linked_worktree": false, "open_workspace_id": "w9"}),
        ];
        assert_eq!(parent_workspace_id(&worktrees), Some("w9".to_string()));
    }

    #[test]
    fn parent_workspace_id_is_none_when_the_parent_has_no_open_workspace() {
        let worktrees = vec![
            json!({"path": "/repos/base", "is_linked_worktree": false, "open_workspace_id": null}),
        ];
        assert_eq!(parent_workspace_id(&worktrees), None);
        assert_eq!(parent_workspace_id(&[]), None);
    }

    #[test]
    fn parent_path_matches_the_non_linked_entry() {
        let worktrees = vec![
            json!({"path": "/repos/a", "is_linked_worktree": true}),
            json!({"path": "/repos/base", "is_linked_worktree": false}),
        ];
        assert_eq!(parent_path(&worktrees), Some("/repos/base".to_string()));
        assert_eq!(parent_path(&[]), None);
    }

    #[test]
    fn workspace_id_from_reads_the_nested_field() {
        let result = json!({"workspace": {"workspace_id": "w9"}});
        assert_eq!(workspace_id_from(&result).unwrap(), "w9");
    }

    #[test]
    fn workspace_id_from_errors_when_missing() {
        let result = json!({"workspace": {}});
        assert!(workspace_id_from(&result).is_err());
    }

    #[test]
    fn workspace_for_checkout_path_matches_exact_path() {
        let workspaces = vec![
            json!({"workspace_id": "w1", "worktree": {"checkout_path": "/repos/a"}}),
            json!({"workspace_id": "w2", "worktree": {"checkout_path": "/repos/b"}}),
        ];
        assert_eq!(
            workspace_for_checkout_path(&workspaces, "/repos/a"),
            Some("w1".to_string())
        );
        assert_eq!(
            workspace_for_checkout_path(&workspaces, "/repos/nope"),
            None
        );
    }

    #[test]
    fn workspace_for_checkout_path_is_none_without_a_worktree_field() {
        let workspaces = vec![json!({"workspace_id": "w1", "label": "@scratch"})];
        assert_eq!(workspace_for_checkout_path(&workspaces, "/repos/a"), None);
    }

    #[test]
    fn herdr_error_text_decodes_a_json_error() {
        let stderr =
            r#"{"id":"2","error":{"code":"already_open","message":"worktree is already open"}}"#;
        assert_eq!(
            herdr_error_text(stderr),
            "already_open: worktree is already open"
        );
    }

    #[test]
    fn herdr_error_text_falls_back_to_raw_trimmed_stderr() {
        assert_eq!(herdr_error_text("  boom: it broke  \n"), "boom: it broke");
        assert_eq!(
            herdr_error_text(r#"{"not":"an error shape"}"#),
            r#"{"not":"an error shape"}"#
        );
    }

    #[test]
    fn code_tab_command_sets_up_the_frame_around_the_tree() {
        assert_eq!(
            code_tab_command("/repos/a", "fix login"),
            vec![
                "emacsclient",
                "-t",
                "--eval",
                "(progn (require (quote treemacs)) (let ((w (selected-window))) \
                 (with-current-buffer (window-buffer w) (cd \"/repos/a\")) \
                 (treemacs-do-add-project-to-workspace \"/repos/a\" \"fix login\") \
                 (treemacs-select-window) (select-window w)))"
            ]
        );
    }

    #[test]
    fn elisp_string_escapes_quotes_and_backslashes() {
        assert_eq!(elisp_string(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
    }

    #[test]
    fn tab_rename_argv_matches_the_cli() {
        assert_eq!(
            tab_rename_argv("w1:t1", "code"),
            vec!["tab", "rename", "w1:t1", "code"]
        );
    }

    #[test]
    fn tab_id_from_reads_the_nested_field() {
        let result = json!({"tab": {"tab_id": "w1:t1"}});
        assert_eq!(tab_id_from(&result).unwrap(), "w1:t1");
    }

    #[test]
    fn tab_id_from_errors_when_missing() {
        let result = json!({"tab": {}});
        assert!(tab_id_from(&result).is_err());
    }

    #[test]
    fn tab_create_argv_matches_the_cli() {
        assert_eq!(
            tab_create_argv("w1", "/repos/a", "fix login"),
            vec![
                "tab",
                "create",
                "--workspace",
                "w1",
                "--cwd",
                "/repos/a",
                "--label",
                "fix login",
                "--focus"
            ]
        );
    }

    #[test]
    fn workspace_create_argv_matches_the_cli() {
        assert_eq!(
            workspace_create_argv("/repos/base", "@scratch"),
            vec![
                "workspace",
                "create",
                "--cwd",
                "/repos/base",
                "--label",
                "@scratch",
                "--focus"
            ]
        );
    }

    #[test]
    fn inner_argv_for_a_tree_has_no_repo_flag() {
        let argv = inner_argv(
            "/usr/local/bin/wt",
            "0199-uuid",
            None,
            Agent::Claude,
            None,
            &["--model".to_string(), "opus".to_string()],
        );
        assert_eq!(
            argv,
            vec![
                "/usr/local/bin/wt",
                "go",
                "0199-uuid",
                "--here",
                "--claude",
                "--",
                "--model",
                "opus"
            ]
        );
    }

    #[test]
    fn inner_argv_for_scratch_adds_repo_and_keeps_the_at_label() {
        let argv = inner_argv(
            "/usr/local/bin/wt",
            "@poking-around",
            Some("myrepo"),
            Agent::Pi,
            Some(&["node".to_string(), "python".to_string()]),
            &[],
        );
        assert_eq!(
            argv,
            vec![
                "/usr/local/bin/wt",
                "go",
                "@poking-around",
                "--repo",
                "myrepo",
                "--here",
                "--pi",
                "--profile",
                "node,python",
            ]
        );
    }

    #[test]
    fn shell_join_quotes_each_token() {
        let argv = vec!["wt".to_string(), "go".to_string(), "it's".to_string()];
        assert_eq!(shell_join(&argv), "'wt' 'go' 'it'\\''s'");
    }
}
