use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// The caller's rank in its own Ghostty window, and the rank of every other
/// tty mapped in that window — a consistent 1..n over the window, computed
/// from one probe. `wt go` writes `others` back into the other sessions'
/// planter state files so a new tab correcting its own rank also corrects
/// whichever tab it pushed down a slot.
pub struct Tabs {
    pub mine: usize,
    pub others: Vec<(String, usize)>,
}

/// Backs `wt __tab-index` (via [`index`]) and the planter renumber (via
/// [`Tabs::others`]). The caller's own tty is always a candidate, even with
/// no planted session running on it yet: `wt go` calls this *before*
/// exec'ing the agent, so it has to count itself.
pub fn probe() -> Result<Tabs> {
    let my_tty = own_tty()?;

    let mut candidates = vec![my_tty.clone()];
    for tty in planter_ttys()? {
        if !candidates.contains(&tty) {
            candidates.push(tty);
        }
    }

    let initial = dump_ghostty()?;
    if initial.is_empty() {
        bail!("Ghostty did not answer");
    }
    let title_of: HashMap<String, String> = initial
        .iter()
        .map(|row| (row.surface_id.clone(), row.title.clone()))
        .collect();

    let run_id = unique_run_id();
    let marker_for = |i: usize| format!("cti-{}-{run_id}-{i}", std::process::id());
    let tty_of_marker: HashMap<String, String> = candidates
        .iter()
        .enumerate()
        .map(|(i, tty)| (marker_for(i), tty.clone()))
        .collect();

    let mut tab_of_tty: HashMap<String, i64> = HashMap::new();
    let mut win_of_tty: HashMap<String, String> = HashMap::new();
    let mut surface_of_tty: HashMap<String, String> = HashMap::new();

    for _pass in 0..6 {
        let mut pending = false;
        for (i, tty) in candidates.iter().enumerate() {
            if tab_of_tty.contains_key(tty) {
                continue;
            }
            pending = true;
            stamp(tty, &marker_for(i));
        }
        if !pending {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));

        for row in dump_ghostty()? {
            let Some(tty) = tty_of_marker.get(&row.title) else {
                continue;
            };
            tab_of_tty.entry(tty.clone()).or_insert(row.tab_index);
            win_of_tty
                .entry(tty.clone())
                .or_insert_with(|| row.window_id.clone());
            surface_of_tty
                .entry(tty.clone())
                .or_insert_with(|| row.surface_id.clone());
        }
    }

    // Put every stamped surface's title back. A session that repaints its
    // own will do so anyway; this is for the idle ones that would otherwise
    // keep the stamp.
    for (tty, surface_id) in &surface_of_tty {
        if let Some(title) = title_of.get(surface_id) {
            stamp(tty, title);
        }
    }

    ranks_for_window(&my_tty, &tab_of_tty, &win_of_tty)
}

/// Prints this tab's own rank; backs `wt __tab-index`.
pub fn index() -> Result<usize> {
    Ok(probe()?.mine)
}

/// Ranks every mapped tty in the caller's own window by ascending Ghostty
/// tab index, deduped by tab index (a split means two ttys share one tab, so
/// tabs are counted, not surfaces). Returns the caller's own rank as `mine`
/// and every other tty in that window as `others`; a tty in a different
/// window never appears in either.
fn ranks_for_window(
    my_tty: &str,
    tab_of_tty: &HashMap<String, i64>,
    win_of_tty: &HashMap<String, String>,
) -> Result<Tabs> {
    let my_win = win_of_tty
        .get(my_tty)
        .context("this tab never reported back")?;

    let mut tab_indices: Vec<i64> = tab_of_tty
        .iter()
        .filter(|(tty, _)| win_of_tty.get(*tty) == Some(my_win))
        .map(|(_, &ti)| ti)
        .collect();
    tab_indices.sort_unstable();
    tab_indices.dedup();

    let rank_of = |ti: i64| tab_indices.iter().position(|&t| t == ti).map(|p| p + 1);

    let mine = tab_of_tty
        .get(my_tty)
        .context("this tab never reported back")
        .and_then(|&ti| rank_of(ti).context("failed to rank this tab"))?;

    let others = tab_of_tty
        .iter()
        .filter(|(tty, _)| tty.as_str() != my_tty && win_of_tty.get(*tty) == Some(my_win))
        .filter_map(|(tty, &ti)| rank_of(ti).map(|r| (tty.clone(), r)))
        .collect();

    Ok(Tabs { mine, others })
}

fn unique_run_id() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Writes an OSC 2 title-set sequence to `tty`. Best-effort: a tty that
/// vanished or refuses the write is simply skipped, matching the zsh
/// version's `2>/dev/null`.
fn stamp(tty: &str, title: &str) {
    if let Ok(mut f) = OpenOptions::new().write(true).open(tty) {
        let _ = write!(f, "\x1b]2;{title}\x07");
    }
}

/// The tty this run belongs to. A `wt go` typed at a shell owns one
/// directly. A Claude Code hook does not — hooks get no terminal of their own —
/// so walk up the parents until one turns up: the claude process that spawned
/// the hook is attached to the tab's tty.
fn own_tty() -> Result<String> {
    let mut pid = std::process::id().to_string();
    for _ in 0..16 {
        let output = Command::new("ps")
            .args(["-o", "tty=,ppid=", "-p", &pid])
            .output()
            .context("running ps for own tty")?;
        let (tty, ppid) = parse_tty_ppid(&String::from_utf8_lossy(&output.stdout));
        if tty.starts_with("ttys") {
            return Ok(format!("/dev/{tty}"));
        }
        // Nothing left to ask: no parent, or ps knows nothing about this pid.
        if ppid.is_empty() || ppid == "1" || ppid == pid {
            break;
        }
        pid = ppid;
    }
    bail!("no tty found in this process's ancestry")
}

/// `(tty, ppid)` out of one `ps -o tty=,ppid=` line. Either can be empty: a pid
/// that has gone away prints nothing at all.
fn parse_tty_ppid(text: &str) -> (String, String) {
    let mut fields = text.split_whitespace();
    (
        fields.next().unwrap_or_default().to_string(),
        fields.next().unwrap_or_default().to_string(),
    )
}

/// Every tty running Claude or the Planter Codex bridge, matched by exact
/// process basename so unplanted Codex sessions do not affect tab ranks.
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
        let mut fields = line.split_whitespace();
        let (Some(tty), Some(comm)) = (fields.next(), fields.next()) else {
            continue;
        };
        if !tty.starts_with("ttys") {
            continue;
        }
        let basename = comm.rsplit('/').next().unwrap_or(comm);
        if matches!(basename, "claude" | "planter-codex-bridge") {
            let dev = format!("/dev/{tty}");
            if !ttys.contains(&dev) {
                ttys.push(dev);
            }
        }
    }
    ttys
}

#[derive(Debug, Clone, PartialEq)]
struct GhosttyRow {
    window_id: String,
    tab_index: i64,
    surface_id: String,
    title: String,
}

/// Each line Ghostty prints is `window_id \t tab_index \t surface_id \t
/// surface_title`. A title can't in practice contain a tab or newline, but
/// parsing splits into at most 4 fields anyway, so an odd title never
/// shifts the other columns.
const GHOSTTY_DUMP_SCRIPT: &str = r#"set d to character id 9
set lf to character id 10
set out to ""
tell application "Ghostty"
  repeat with w in windows
    set wid to id of w
    repeat with t in tabs of w
      set ti to index of t
      repeat with s in terminals of t
        set out to out & wid & d & ti & d & (id of s) & d & (name of s) & lf
      end repeat
    end repeat
  end repeat
end tell
return out"#;

fn dump_ghostty() -> Result<Vec<GhosttyRow>> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(GHOSTTY_DUMP_SCRIPT)
        .output()
        .context("running osascript")?;
    if !output.status.success() {
        bail!(
            "osascript exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_dump(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_dump(text: &str) -> Vec<GhosttyRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        let [window_id, tab_index, surface_id, title] = parts[..] else {
            continue;
        };
        let Ok(tab_index) = tab_index.parse::<i64>() else {
            continue;
        };
        rows.push(GhosttyRow {
            window_id: window_id.to_string(),
            tab_index,
            surface_id: surface_id.to_string(),
            title: title.to_string(),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dump_reads_tab_separated_rows() {
        let text = "1\t1\t10\tfirst\n1\t2\t11\tsecond\n2\t1\t12\tthird\n";
        let rows = parse_dump(text);
        assert_eq!(
            rows,
            vec![
                GhosttyRow {
                    window_id: "1".into(),
                    tab_index: 1,
                    surface_id: "10".into(),
                    title: "first".into(),
                },
                GhosttyRow {
                    window_id: "1".into(),
                    tab_index: 2,
                    surface_id: "11".into(),
                    title: "second".into(),
                },
                GhosttyRow {
                    window_id: "2".into(),
                    tab_index: 1,
                    surface_id: "12".into(),
                    title: "third".into(),
                },
            ]
        );
    }

    #[test]
    fn parse_dump_keeps_extra_tabs_in_the_title() {
        let rows = parse_dump("1\t1\t10\ttitle\twith\ttabs\n");
        assert_eq!(rows[0].title, "title\twith\ttabs");
    }

    #[test]
    fn parse_dump_skips_malformed_lines() {
        assert!(parse_dump("only\tthree\tfields\n").is_empty());
        assert!(parse_dump("1\tnot-a-number\t10\ttitle\n").is_empty());
    }

    #[test]
    fn parse_planter_ttys_matches_exact_session_processes_and_deduplicates() {
        let text = "ttys001 claude\nttys002 codex\nttys003 /opt/homebrew/bin/claude\nttys004 /opt/bin/planter-codex-bridge\nttys004 planter-codex-bridge\nconsole claude\n";
        assert_eq!(
            parse_planter_ttys(text),
            vec![
                "/dev/ttys001".to_string(),
                "/dev/ttys003".to_string(),
                "/dev/ttys004".to_string(),
            ]
        );
    }

    #[test]
    fn ranks_for_window_counts_tabs_not_surfaces_in_my_window() {
        let mut tab_of_tty = HashMap::new();
        let mut win_of_tty = HashMap::new();

        // Two ttys share tab 3 via a split; another tty is tab 5; a tty in
        // a different window must not affect the ranking.
        for (tty, win, tab) in [
            ("/dev/ttys001", "1", 3),
            ("/dev/ttys002", "1", 3),
            ("/dev/ttys003", "1", 5),
            ("/dev/ttys004", "2", 1),
        ] {
            tab_of_tty.insert(tty.to_string(), tab);
            win_of_tty.insert(tty.to_string(), win.to_string());
        }

        assert_eq!(
            ranks_for_window("/dev/ttys001", &tab_of_tty, &win_of_tty)
                .unwrap()
                .mine,
            1
        );
        assert_eq!(
            ranks_for_window("/dev/ttys002", &tab_of_tty, &win_of_tty)
                .unwrap()
                .mine,
            1
        );
        assert_eq!(
            ranks_for_window("/dev/ttys003", &tab_of_tty, &win_of_tty)
                .unwrap()
                .mine,
            2
        );
    }

    #[test]
    fn ranks_for_window_others_excludes_caller_and_other_windows() {
        let mut tab_of_tty = HashMap::new();
        let mut win_of_tty = HashMap::new();

        for (tty, win, tab) in [
            ("/dev/ttys001", "1", 3),
            ("/dev/ttys002", "1", 3),
            ("/dev/ttys003", "1", 5),
            ("/dev/ttys004", "2", 1),
        ] {
            tab_of_tty.insert(tty.to_string(), tab);
            win_of_tty.insert(tty.to_string(), win.to_string());
        }

        let mut others = ranks_for_window("/dev/ttys001", &tab_of_tty, &win_of_tty)
            .unwrap()
            .others;
        others.sort();
        assert_eq!(
            others,
            vec![
                ("/dev/ttys002".to_string(), 1),
                ("/dev/ttys003".to_string(), 2),
            ]
        );
    }

    #[test]
    fn parse_tty_ppid_reads_both_fields_and_tolerates_missing_ones() {
        assert_eq!(
            parse_tty_ppid("ttys005  6806\n"),
            ("ttys005".to_string(), "6806".to_string())
        );
        // A ttyless process still reports its parent, which is what the walk needs.
        assert_eq!(
            parse_tty_ppid("??  13252\n"),
            ("??".to_string(), "13252".to_string())
        );
        // A pid that has gone away prints nothing.
        assert_eq!(parse_tty_ppid(""), (String::new(), String::new()));
    }

    #[test]
    fn ranks_for_window_errors_when_my_tty_never_reported() {
        let tab_of_tty = HashMap::new();
        let win_of_tty = HashMap::new();
        assert!(ranks_for_window("/dev/ttys009", &tab_of_tty, &win_of_tty).is_err());
    }
}
