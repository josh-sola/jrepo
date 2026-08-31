//! Resolves a [`Hook`] to its effect: a builtin calls the matching wt code
//! directly, a `cmd` spawns the configured command. Every failure here —
//! unconfigured, non-zero exit, unparseable output, or timeout — means "this
//! hook produced nothing"; a launch must never fail because a hook did.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::color;
use crate::config::Hook;
use crate::planter;
use crate::tab::{self, Tabs};

const HOOK_TIMEOUT: Duration = Duration::from_secs(2);

/// Context a `cmd` hook receives as environment, threaded through explicitly
/// rather than read from globals.
pub struct Context<'a> {
    pub tree_path: &'a Path,
    pub repo: &'a str,
    pub label: &'a str,
    pub color_hex: &'a str,
    pub primary_hex: &'a str,
    pub text_hex: &'a str,
}

/// Runs the get-position hook. `None` when the hook is unconfigured or fails.
pub fn get_position(hook: Option<&Hook>, ctx: &Context) -> Option<Tabs> {
    match hook? {
        Hook::Builtin(name) if name == "ghostty-tab" => tab::probe().ok(),
        Hook::Builtin(other) => {
            eprintln!("wt: unknown get-position builtin '{other}'");
            None
        }
        Hook::Cmd(argv) => {
            let stdout = run_cmd(argv, ctx)?;
            let text = String::from_utf8_lossy(&stdout);
            match text.trim().parse::<usize>() {
                Ok(mine) => Some(Tabs {
                    mine,
                    others: Vec::new(),
                }),
                Err(_) => {
                    eprintln!(
                        "wt: get-position hook printed non-integer output: {:?}",
                        text.trim()
                    );
                    None
                }
            }
        }
    }
}

/// Runs the renumber-peers hook with the peers `get_position` reported.
pub fn renumber_peers(hook: Option<&Hook>, tabs: &Tabs, ctx: &Context) {
    let Some(hook) = hook else { return };
    match hook {
        Hook::Builtin(name) if name == "planter-state" => planter::renumber(tabs),
        Hook::Builtin(other) => eprintln!("wt: unknown renumber-peers builtin '{other}'"),
        Hook::Cmd(argv) => {
            run_cmd(argv, ctx);
        }
    }
}

/// Runs the set-background hook.
pub fn set_background(hook: Option<&Hook>, entry: &color::PaletteEntry, ctx: &Context) {
    let Some(hook) = hook else { return };
    match hook {
        Hook::Builtin(name) if name == "osc11" => color::set_background(entry),
        Hook::Builtin(other) => eprintln!("wt: unknown set-background builtin '{other}'"),
        Hook::Cmd(argv) => {
            run_cmd(argv, ctx);
        }
    }
}

/// Spawns `argv[0] argv[1..]` with wt's context in its environment, waits up
/// to [`HOOK_TIMEOUT`], and returns its stdout on a clean exit. Kills the
/// child and returns `None` past the timeout.
fn run_cmd(argv: &[String], ctx: &Context) -> Option<Vec<u8>> {
    let (program, rest) = argv.split_first()?;
    let program = expand_tilde(program);

    let mut command = Command::new(&program);
    command
        .args(rest)
        .env("WT_TREE_PATH", ctx.tree_path)
        .env("WT_REPO", ctx.repo)
        .env("WT_LABEL", ctx.label)
        .env("WT_COLOR_HEX", ctx.color_hex)
        .env("WT_COLOR_PRIMARY_HEX", ctx.primary_hex)
        .env("WT_COLOR_TEXT_HEX", ctx.text_hex)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("wt: hook '{program}' failed to start: {e}");
            return None;
        }
    };
    let pid = child.id();

    // `wait_with_output` drains both pipes concurrently on its own, so a
    // chatty child can't deadlock this thread while it blocks on the
    // timeout below.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(HOOK_TIMEOUT) {
        Ok(Ok(output)) => {
            if !output.status.success() {
                eprintln!(
                    "wt: hook '{program}' exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                return None;
            }
            Some(output.stdout)
        }
        Ok(Err(e)) => {
            eprintln!("wt: hook '{program}' failed: {e}");
            None
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill(pid);
            eprintln!("wt: hook '{program}' timed out after {HOOK_TIMEOUT:?}");
            None
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => None,
    }
}

fn kill(pid: u32) {
    let _ = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// A config file is the kind of place someone writes `~/bin/foo`; only the
/// program itself is expanded, matching a shell's own tilde expansion.
fn expand_tilde(arg: &str) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return arg.to_string();
    };
    if arg == "~" {
        return home;
    }
    match arg.strip_prefix("~/") {
        Some(rest) => format!("{home}/{rest}"),
        None => arg.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    fn ctx(tree_path: &Path) -> Context<'_> {
        Context {
            tree_path,
            repo: "monorepo",
            label: "fix login",
            color_hex: "#123456",
            primary_hex: "#abcdef",
            text_hex: "#fedcba",
        }
    }

    fn sh(script: &str) -> Hook {
        Hook::Cmd(vec!["sh".to_string(), "-c".to_string(), script.to_string()])
    }

    #[test]
    fn cmd_hook_receives_all_context_env_vars() {
        let tree_path = PathBuf::from("/tmp/wt-features-test");
        let hook = sh(
            "printf '%s %s %s %s %s\\n' \"$WT_TREE_PATH\" \"$WT_REPO\" \"$WT_LABEL\" \"$WT_COLOR_HEX\" \"$WT_COLOR_PRIMARY_HEX\"; printf '%s\\n' \"$WT_COLOR_TEXT_HEX\"",
        );
        let stdout = run_cmd(
            match &hook {
                Hook::Cmd(argv) => argv,
                Hook::Builtin(_) => unreachable!(),
            },
            &ctx(&tree_path),
        )
        .expect("hook should succeed");
        let text = String::from_utf8_lossy(&stdout);
        assert_eq!(
            text,
            "/tmp/wt-features-test monorepo fix login #123456 #abcdef\n#fedcba\n"
        );
    }

    #[test]
    fn cmd_get_position_parses_integer_stdout() {
        let tree_path = PathBuf::from("/tmp/wt-features-test");
        let hook = sh("echo 7");
        let tabs = get_position(Some(&hook), &ctx(&tree_path)).expect("hook should succeed");
        assert_eq!(tabs.mine, 7);
        assert!(tabs.others.is_empty());
    }

    #[test]
    fn cmd_get_position_non_zero_exit_yields_none() {
        let tree_path = PathBuf::from("/tmp/wt-features-test");
        let hook = sh("exit 1");
        assert!(get_position(Some(&hook), &ctx(&tree_path)).is_none());
    }

    #[test]
    fn cmd_get_position_unparseable_stdout_yields_none() {
        let tree_path = PathBuf::from("/tmp/wt-features-test");
        let hook = sh("echo not-a-number");
        assert!(get_position(Some(&hook), &ctx(&tree_path)).is_none());
    }

    #[test]
    fn cmd_get_position_past_timeout_yields_none_and_does_not_hang() {
        let tree_path = PathBuf::from("/tmp/wt-features-test");
        let hook = sh("sleep 5");
        let start = Instant::now();
        let result = get_position(Some(&hook), &ctx(&tree_path));
        assert!(result.is_none());
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "hook should have been killed near the {HOOK_TIMEOUT:?} timeout, took {:?}",
            start.elapsed()
        );
    }
}
