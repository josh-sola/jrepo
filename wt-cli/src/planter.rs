use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::color;
use crate::tmux::Windows;

const RESERVED_FILES: [&str; 3] = ["order.json", "prefs.json", "overlay-position.json"];
const RESOLVE_COLOR_TIMEOUT: Duration = Duration::from_secs(2);

/// Asks `planter` to resolve the tree's launch color. Planter is a hard
/// dependency for `wt go`/`wt llm`: a missing binary, a timeout, a nonzero
/// exit, or malformed stdout all fail the launch rather than falling back to
/// a hash, so nothing here is best-effort.
pub fn resolve_color() -> Result<&'static color::PaletteEntry> {
    resolve_color_command("planter", &["--resolve-color"])
}

fn resolve_color_command(program: &str, args: &[&str]) -> Result<&'static color::PaletteEntry> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            anyhow::anyhow!("`planter` is not on PATH; install it before `wt launch`")
        } else {
            anyhow::anyhow!("could not start `planter --resolve-color`: {error}")
        }
    })?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(RESOLVE_COLOR_TIMEOUT) {
        Ok(Ok(output)) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if stderr.is_empty() {
                    bail!("`planter --resolve-color` exited with {}", output.status);
                }
                bail!(
                    "`planter --resolve-color` exited with {}: {stderr}",
                    output.status
                );
            }
            parse_resolved_color(&output.stdout)
        }
        Ok(Err(error)) => Err(error).context("waiting for `planter --resolve-color`"),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill(pid);
            bail!("`planter --resolve-color` timed out after {RESOLVE_COLOR_TIMEOUT:?}");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            bail!("`planter --resolve-color` stopped before returning output")
        }
    }
}

fn parse_resolved_color(stdout: &[u8]) -> Result<&'static color::PaletteEntry> {
    let output =
        std::str::from_utf8(stdout).context("`planter --resolve-color` wrote non-UTF-8 stdout")?;
    let Some(name) = output.strip_suffix('\n') else {
        bail!("`planter --resolve-color` must print one palette name followed by a newline");
    };
    color::lookup(name)
        .with_context(|| format!("`planter --resolve-color` printed invalid stdout: {output:?}"))
}

fn kill(pid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-9", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Rewrites the `tab` field of every other live session's planter state
/// file to the rank `tmux::probe` just computed for it, so a new launch can
/// immediately correct another agent window it pushed down. Best effort, like
/// the probe itself: nothing here may fail or block a launch.
pub fn renumber(windows: &Windows) {
    if let Err(e) = try_renumber(windows) {
        eprintln!("wt: planter renumber skipped: {e:#}");
    }
}

fn try_renumber(windows: &Windows) -> Result<()> {
    if windows.others.is_empty() {
        return Ok(());
    }

    let dir = state_dir();
    let read_dir = match fs::read_dir(&dir) {
        Ok(read_dir) => read_dir,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };

    let ranks: HashMap<String, usize> = windows.others.iter().cloned().collect();

    let mut candidates = Vec::new();
    for entry in read_dir {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_state_file(name) {
            continue;
        }
        if let Some(candidate) = read_candidate(&path) {
            candidates.push(candidate);
        }
    }

    for (path, tab) in plan_writes(&candidates, &ranks) {
        if let Err(e) = write_tab(&path, tab) {
            eprintln!("wt: planter renumber skipped {}: {e:#}", path.display());
        }
    }
    Ok(())
}

fn state_dir() -> PathBuf {
    let planter_state_dir = std::env::var("PLANTER_STATE_DIR").ok();
    let claude_planter_dir = std::env::var("CLAUDE_PLANTER_DIR").ok();
    let home = std::env::var("HOME").ok();
    state_dir_from_vars(
        planter_state_dir.as_deref(),
        claude_planter_dir.as_deref(),
        home.as_deref(),
    )
}

fn state_dir_from_vars(
    planter_state_dir: Option<&str>,
    claude_planter_dir: Option<&str>,
    home: Option<&str>,
) -> PathBuf {
    if let Some(dir) = planter_state_dir {
        return PathBuf::from(dir);
    }
    if let Some(dir) = claude_planter_dir {
        return PathBuf::from(dir);
    }
    PathBuf::from(home.unwrap_or_default())
        .join(".claude")
        .join("planter")
}

/// A `*.json` file that isn't one of the planter's own bookkeeping files —
/// i.e. a per-session state file.
fn is_state_file(name: &str) -> bool {
    name.ends_with(".json") && !RESERVED_FILES.contains(&name)
}

/// One planter state file's tab-relevant fields, gathered by reading the
/// file and resolving its `pid` to a tty. `tty` is `None` when the pid is
/// gone or unreadable; `current_tab` is `None` for a missing or `null` field.
struct Candidate {
    path: PathBuf,
    tty: Option<String>,
    current_tab: Option<i64>,
}

fn read_candidate(path: &Path) -> Option<Candidate> {
    let bytes = fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let pid = value.get("pid")?.as_i64()?;
    Some(Candidate {
        path: path.to_path_buf(),
        tty: pid_to_tty(pid),
        current_tab: value.get("tab").and_then(Value::as_i64),
    })
}

fn pid_to_tty(pid: i64) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let tty = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !tty.starts_with("ttys") {
        return None;
    }
    Some(format!("/dev/{tty}"))
}

/// The pure mapping at the heart of the renumber: given each candidate
/// file's resolved tty and current `tab` value, and the rank each tty holds
/// in the caller's tmux session, decide which files need a new `tab` written.
/// A candidate whose tty isn't in `ranks`, or whose current tab already
/// matches its rank, produces no write.
fn plan_writes(candidates: &[Candidate], ranks: &HashMap<String, usize>) -> Vec<(PathBuf, usize)> {
    candidates
        .iter()
        .filter_map(|c| {
            let tty = c.tty.as_deref()?;
            let rank = *ranks.get(tty)?;
            if c.current_tab == Some(rank as i64) {
                None
            } else {
                Some((c.path.clone(), rank))
            }
        })
        .collect()
}

/// Rewrites just the `tab` field, preserving every other field, atomically:
/// a temp file in the same directory, then a rename. The hook that owns
/// these files writes the same way, so a torn read is impossible.
fn write_tab(path: &Path, tab: usize) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut value: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    value
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", path.display()))?
        .insert("tab".to_string(), Value::from(tab));

    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no file name", path.display()))?;
    // Suffixed with the pid, like the hook's own temp files: two writers must
    // never land on the same temp path and hand each other half a file.
    let tmp = dir.join(format!("{file_name}.tmp.{}", std::process::id()));

    let mut bytes =
        serde_json::to_vec(&value).with_context(|| format!("serializing {file_name}"))?;
    bytes.push(b'\n');
    let mut file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).with_context(|| format!("renaming {} into place", tmp.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn resolve_with_shell(script: &str) -> Result<&'static color::PaletteEntry> {
        resolve_color_command("/bin/sh", &["-c", script])
    }

    #[test]
    fn resolver_accepts_every_palette_token() {
        for entry in &color::PALETTE {
            assert_eq!(
                resolve_with_shell(&format!("printf '%s\\n' {}", entry.name)).unwrap(),
                entry
            );
        }
    }

    #[test]
    fn resolver_rejects_invalid_or_malformed_stdout() {
        for script in [
            "printf 'notacolor\\n'",
            "printf 'cyan\\nextra\\n'",
            "printf 'cyan\\n\\n'",
            "printf cyan",
        ] {
            assert!(resolve_with_shell(script).is_err(), "script was: {script}");
        }
    }

    #[test]
    fn resolver_reports_nonzero_status_with_stderr() {
        let error = resolve_with_shell("printf 'the resolver failed' >&2; exit 23").unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("exit status: 23"), "{text}");
        assert!(text.contains("the resolver failed"), "{text}");
    }

    #[test]
    fn resolver_reports_a_missing_program() {
        let error = resolve_color_command("/nonexistent/wt-planter", &[]).unwrap_err();
        assert!(error.to_string().contains("not on PATH"), "{error:#}");
    }

    #[test]
    fn resolver_times_out_promptly() {
        let start = Instant::now();
        let error = resolve_with_shell("exec sleep 5").unwrap_err();
        assert!(error.to_string().contains("timed out"), "{error:#}");
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "resolver took {:?}",
            start.elapsed()
        );
    }

    fn candidate(path: &str, tty: Option<&str>, current_tab: Option<i64>) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            tty: tty.map(String::from),
            current_tab,
        }
    }

    #[test]
    fn plan_writes_rewrites_a_changed_rank() {
        let ranks = HashMap::from([("/dev/ttys002".to_string(), 3)]);
        let candidates = vec![candidate("/state/a.json", Some("/dev/ttys002"), Some(2))];
        assert_eq!(
            plan_writes(&candidates, &ranks),
            vec![(PathBuf::from("/state/a.json"), 3)]
        );
    }

    #[test]
    fn plan_writes_skips_a_rank_that_already_matches() {
        let ranks = HashMap::from([("/dev/ttys002".to_string(), 3)]);
        let candidates = vec![candidate("/state/a.json", Some("/dev/ttys002"), Some(3))];
        assert!(plan_writes(&candidates, &ranks).is_empty());
    }

    #[test]
    fn plan_writes_skips_a_pid_whose_tty_is_not_in_the_map() {
        let ranks = HashMap::from([("/dev/ttys002".to_string(), 3)]);
        let candidates = vec![candidate("/state/a.json", Some("/dev/ttys009"), Some(1))];
        assert!(plan_writes(&candidates, &ranks).is_empty());
    }

    #[test]
    fn plan_writes_skips_a_candidate_with_no_resolved_tty() {
        let ranks = HashMap::from([("/dev/ttys002".to_string(), 3)]);
        let candidates = vec![candidate("/state/a.json", None, Some(1))];
        assert!(plan_writes(&candidates, &ranks).is_empty());
    }

    #[test]
    fn plan_writes_adopts_a_rank_for_a_null_tab() {
        let ranks = HashMap::from([("/dev/ttys002".to_string(), 3)]);
        let candidates = vec![candidate("/state/a.json", Some("/dev/ttys002"), None)];
        assert_eq!(
            plan_writes(&candidates, &ranks),
            vec![(PathBuf::from("/state/a.json"), 3)]
        );
    }

    #[test]
    fn is_state_file_skips_reserved_names() {
        assert!(!is_state_file("order.json"));
        assert!(!is_state_file("prefs.json"));
        assert!(!is_state_file("overlay-position.json"));
        assert!(!is_state_file("label-hook"));
        assert!(is_state_file("42c054f0-35bb-4624-8f32-40ee8f4fb34a.json"));
    }

    #[test]
    fn state_dir_prefers_the_current_variable_then_the_legacy_one() {
        assert_eq!(
            state_dir_from_vars(Some("/current"), Some("/legacy"), Some("/home/me")),
            PathBuf::from("/current")
        );
        assert_eq!(
            state_dir_from_vars(None, Some("/legacy"), Some("/home/me")),
            PathBuf::from("/legacy")
        );
        assert_eq!(
            state_dir_from_vars(None, None, Some("/home/me")),
            PathBuf::from("/home/me/.claude/planter")
        );
    }
}
