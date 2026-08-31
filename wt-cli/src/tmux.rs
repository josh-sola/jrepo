use std::collections::HashMap;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The caller's rank among supported agent windows in its tmux session, and
/// the rank of every other planted pane. `wt go` writes `others` back into
/// those sessions' Planter state files so a new window correcting its own rank
/// also corrects the sessions it pushed down.
#[derive(Debug)]
pub struct Windows {
    pub mine: usize,
    pub others: Vec<(String, usize)>,
}

const PANE_FORMAT: &str = "#{window_id}\t#{window_index}\t#{pane_id}\t#{pane_tty}";

/// Backs `wt __window-index` (via [`index`]) and Planter peer renumbering
/// (via [`Windows::others`]). `wt go` probes before it execs an agent, so the
/// caller pane is a candidate even when its tty has no supported agent yet.
pub fn probe() -> Result<Windows> {
    let pane_id = std::env::var("TMUX_PANE")
        .ok()
        .filter(|pane_id| !pane_id.is_empty())
        .context("TMUX_PANE is not set")?;
    let session_id = current_session(&pane_id)?;
    let panes = session_panes(&session_id)?;
    ranks_for_panes(&pane_id, &planter_ttys()?, &panes)
}

pub fn index() -> Result<usize> {
    Ok(probe()?.mine)
}

fn current_session(pane_id: &str) -> Result<String> {
    let output = run_tmux(&["display-message", "-p", "-t", pane_id, "#{session_id}"])?;
    let session_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if session_id.is_empty() {
        bail!("tmux did not report a session for pane {pane_id}");
    }
    Ok(session_id)
}

fn session_panes(session_id: &str) -> Result<Vec<PaneRow>> {
    let output = run_tmux(&list_panes_args(session_id))?;
    Ok(parse_pane_rows(&String::from_utf8_lossy(&output.stdout)))
}

fn list_panes_args(session_id: &str) -> [&str; 6] {
    ["list-panes", "-s", "-t", session_id, "-F", PANE_FORMAT]
}

fn run_tmux(args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .with_context(|| format!("running tmux {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "tmux {} exited with {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

#[derive(Debug, Clone, PartialEq)]
struct PaneRow {
    window_id: String,
    window_index: i64,
    pane_id: String,
    pane_tty: String,
}

fn parse_pane_rows(text: &str) -> Vec<PaneRow> {
    text.lines().filter_map(parse_pane_row).collect()
}

fn parse_pane_row(line: &str) -> Option<PaneRow> {
    let mut fields = line.split('\t');
    let (Some(window_id), Some(window_index), Some(pane_id), Some(pane_tty), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if window_id.is_empty() || pane_id.is_empty() || pane_tty.is_empty() {
        return None;
    }
    Some(PaneRow {
        window_id: window_id.to_string(),
        window_index: window_index.parse().ok()?,
        pane_id: pane_id.to_string(),
        pane_tty: pane_tty.to_string(),
    })
}

/// Dense-ranks candidate windows by tmux's numeric window index. The caller
/// pane is a candidate independently of its tty; every other candidate must
/// be running a supported Planter agent.
fn ranks_for_panes(
    caller_pane: &str,
    planted_ttys: &[String],
    panes: &[PaneRow],
) -> Result<Windows> {
    let caller = panes
        .iter()
        .find(|pane| pane.pane_id == caller_pane)
        .context("tmux did not report the caller pane")?;

    let candidates = panes
        .iter()
        .filter(|pane| pane.pane_id == caller_pane || planted_ttys.contains(&pane.pane_tty))
        .collect::<Vec<_>>();

    let mut index_by_window = HashMap::new();
    for pane in candidates {
        index_by_window
            .entry(pane.window_id.as_str())
            .or_insert(pane.window_index);
    }
    let mut windows = index_by_window.into_iter().collect::<Vec<_>>();
    windows.sort_unstable_by(|(left_id, left_index), (right_id, right_index)| {
        left_index
            .cmp(right_index)
            .then_with(|| left_id.cmp(right_id))
    });
    let rank_by_window = windows
        .iter()
        .enumerate()
        .map(|(position, (window_id, _))| (*window_id, position + 1))
        .collect::<HashMap<_, _>>();

    let mine = rank_by_window
        .get(caller.window_id.as_str())
        .copied()
        .context("failed to rank the caller pane's window")?;
    let others = panes
        .iter()
        .filter(|pane| pane.pane_id != caller_pane && planted_ttys.contains(&pane.pane_tty))
        .filter_map(|pane| {
            rank_by_window
                .get(pane.window_id.as_str())
                .map(|rank| (pane.pane_tty.clone(), *rank))
        })
        .collect();

    Ok(Windows { mine, others })
}

/// Every tty running Pi, Claude, or the Planter Codex bridge, matched by exact
/// process basename so unplanted Codex sessions do not affect window ranks.
fn planter_ttys() -> Result<Vec<String>> {
    let output = Command::new("ps")
        .args(["-Ao", "tty=,comm="])
        .output()
        .context("running ps for planter ttys")?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_planter_ttys(&text))
}

fn parse_planter_ttys(text: &str) -> Vec<String> {
    let mut ttys = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let Some(separator) = line.find(char::is_whitespace) else {
            continue;
        };
        let (tty, comm) = line.split_at(separator);
        let comm = comm.trim_start();
        if !tty.starts_with("ttys") || comm.is_empty() {
            continue;
        }
        let basename = comm.rsplit('/').next().unwrap_or(comm);
        if matches!(basename, "pi" | "claude" | "planter-codex-bridge") {
            let dev = format!("/dev/{tty}");
            if !ttys.contains(&dev) {
                ttys.push(dev);
            }
        }
    }
    ttys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Vec<PaneRow> {
        parse_pane_rows(text)
    }

    #[test]
    fn list_panes_arguments_scope_input_to_the_current_session() {
        assert_eq!(
            list_panes_args("$12"),
            [
                "list-panes",
                "-s",
                "-t",
                "$12",
                "-F",
                "#{window_id}\t#{window_index}\t#{pane_id}\t#{pane_tty}",
            ]
        );
    }

    #[test]
    fn ranks_include_the_caller_before_agent_exec() {
        let windows = ranks_for_panes(
            "%1",
            &[],
            &rows("@1\t7\t%1\t/dev/ttys001\n@2\t9\t%2\t/dev/ttys002\n"),
        )
        .unwrap();

        assert_eq!(windows.mine, 1);
        assert!(windows.others.is_empty());
    }

    #[test]
    fn ranks_splits_once_and_gives_them_a_shared_rank() {
        let planted = vec!["/dev/ttys002".to_string(), "/dev/ttys003".to_string()];
        let windows = ranks_for_panes(
            "%1",
            &planted,
            &rows("@1\t3\t%1\t/dev/ttys001\n@1\t3\t%2\t/dev/ttys002\n@2\t5\t%3\t/dev/ttys003\n"),
        )
        .unwrap();

        assert_eq!(windows.mine, 1);
        assert_eq!(
            windows.others,
            vec![
                ("/dev/ttys002".to_string(), 1),
                ("/dev/ttys003".to_string(), 2)
            ]
        );
    }

    #[test]
    fn ranks_skip_non_agent_panes() {
        let planted = vec!["/dev/ttys003".to_string()];
        let windows = ranks_for_panes(
            "%1",
            &planted,
            &rows("@1\t4\t%1\t/dev/ttys001\n@2\t1\t%2\t/dev/ttys002\n@3\t7\t%3\t/dev/ttys003\n"),
        )
        .unwrap();

        assert_eq!(windows.mine, 1);
        assert_eq!(windows.others, vec![("/dev/ttys003".to_string(), 2)]);
    }

    #[test]
    fn ranks_make_gapped_tmux_indices_dense() {
        let planted = vec!["/dev/ttys003".to_string()];
        let windows = ranks_for_panes(
            "%1",
            &planted,
            &rows("@1\t3\t%1\t/dev/ttys001\n@2\t42\t%3\t/dev/ttys003\n"),
        )
        .unwrap();

        assert_eq!(windows.mine, 1);
        assert_eq!(windows.others, vec![("/dev/ttys003".to_string(), 2)]);
    }

    #[test]
    fn parse_pane_rows_skips_malformed_rows() {
        let parsed = rows(
            "@1\t3\t%1\t/dev/ttys001\nmalformed\n@2\tnot-a-number\t%2\t/dev/ttys002\n@3\t5\t%3\t\n@4\t6\t%4\t/dev/ttys004\textra\n",
        );
        assert_eq!(
            parsed,
            vec![PaneRow {
                window_id: "@1".to_string(),
                window_index: 3,
                pane_id: "%1".to_string(),
                pane_tty: "/dev/ttys001".to_string(),
            }]
        );
    }

    #[test]
    fn ranks_error_when_the_caller_is_missing() {
        let err = ranks_for_panes("%1", &[], &rows("@2\t3\t%2\t/dev/ttys002\n")).unwrap_err();
        assert!(err.to_string().contains("caller pane"));
    }

    #[test]
    fn parse_planter_ttys_matches_exact_session_processes_and_deduplicates() {
        let text = "ttys001 claude\nttys002 codex\nttys003 /opt/homebrew/bin/claude\nttys004 /opt/bin/planter-codex-bridge\nttys004 planter-codex-bridge\nttys005 /opt/bin/pi\nttys006 api\n  ttys007 /Applications/My Tools/claude\nconsole pi\n";
        assert_eq!(
            parse_planter_ttys(text),
            vec![
                "/dev/ttys001".to_string(),
                "/dev/ttys003".to_string(),
                "/dev/ttys004".to_string(),
                "/dev/ttys005".to_string(),
                "/dev/ttys007".to_string(),
            ]
        );
    }
}
