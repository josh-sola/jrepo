//! The launch picker: a ratatui screen that runs whenever `wt go` has no
//! selector and produces a full launch request. `cmd_launch` continues from
//! that request exactly as if its fields came from CLI flags.

mod state;
mod view;

use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use uuid::Uuid;

use crate::agent::Agent;
use crate::{config, store};

pub struct LaunchRequest {
    pub selector: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub onto: Option<String>,
    pub profile: Option<Vec<String>>,
    pub agent: Agent,
    pub args: Vec<String>,
}

/// Initial field values, drawn from whatever CLI flags accompanied a
/// selector-less `wt go`. `--claude` alongside no selector, for instance,
/// opens the picker with Claude preselected.
pub struct Seed {
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub onto: Option<String>,
    pub profile: Option<Vec<String>>,
    pub agent: Agent,
    pub args: Vec<String>,
}

pub fn pick(
    store: &store::Store,
    config: &config::Config,
    cwd_repo: Option<&str>,
    seed: Seed,
) -> Result<Option<LaunchRequest>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("no worktree selector given and no terminal to pick one from");
    }

    let rows = state::build_rows(&store.trees, cwd_repo);
    let repos: Vec<String> = store.repos.keys().cloned().collect();
    let mut app = state::State::new(rows, repos, cwd_repo, seed);

    install_panic_hook();
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("initializing the terminal")?;

    run_loop(&mut terminal, &mut app, store, config)
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        execute!(io::stdout(), EnterAlternateScreen).context("entering the alternate screen")?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // The agent execs into this same terminal right after, so leftover
        // raw mode or the alternate screen would break its session.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// A panic mid-picker must still restore the terminal, or the shell it
/// returns to is left in raw mode with the alternate screen up.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        previous(info);
    }));
}

/// What the preview worker needs for one tree; sent whole so the worker
/// never has to reach back into the store from its own thread.
struct PreviewRequest {
    id: Uuid,
    tree: store::Tree,
}

/// Computes the slow, git-backed half of the preview on its own thread so a
/// highlighted-but-unvisited tree never blocks a keypress. Runs until the
/// main loop drops its request sender.
fn preview_worker(
    config: config::Config,
    requests: mpsc::Receiver<PreviewRequest>,
    results: mpsc::Sender<(Uuid, String)>,
) {
    while let Ok(first) = requests.recv() {
        let request = drain_to_latest(first, &requests);
        let text = crate::preview_git_lines(&config, &request.tree).join("\n");
        if results.send((request.id, text)).is_err() {
            return;
        }
    }
}

/// Collapses everything already queued down to the most recent item, so a
/// burst of keypresses doesn't make the worker compute previews for trees
/// the user has already scrolled past.
fn drain_to_latest<T>(first: T, rx: &mpsc::Receiver<T>) -> T {
    let mut latest = first;
    while let Ok(item) = rx.try_recv() {
        latest = item;
    }
    latest
}

fn needs_request(id: Uuid, cache: &HashMap<Uuid, String>, pending: Option<Uuid>) -> bool {
    !cache.contains_key(&id) && pending != Some(id)
}

/// The header is rebuilt every frame because it is cheap; only the git tail
/// waits on the worker.
fn render_preview(tree: Option<&store::Tree>, git_cache: &HashMap<Uuid, String>) -> String {
    let Some(tree) = tree else {
        return String::new();
    };
    let header = crate::preview_header_lines(tree).join("\n");
    let tail = git_cache.get(&tree.id).map(String::as_str).unwrap_or("…");
    format!("{header}\n{tail}")
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut state::State,
    store: &store::Store,
    config: &config::Config,
) -> Result<Option<LaunchRequest>> {
    let (req_tx, req_rx) = mpsc::channel::<PreviewRequest>();
    let (res_tx, res_rx) = mpsc::channel::<(Uuid, String)>();
    let worker_config = config.clone();
    // Not joined: a git call may still be in flight, and the process is
    // about to exec the agent or exit.
    thread::spawn(move || preview_worker(worker_config, req_rx, res_tx));

    let mut git_cache: HashMap<Uuid, String> = HashMap::new();
    let mut pending: Option<Uuid> = None;

    loop {
        for (id, text) in res_rx.try_iter() {
            git_cache.insert(id, text);
            if pending == Some(id) {
                pending = None;
            }
        }

        let selected = app
            .filtered
            .get(app.selected)
            .and_then(|&i| store.trees.iter().find(|t| t.id == app.rows[i].id));

        if let Some(tree) = selected
            && needs_request(tree.id, &git_cache, pending)
        {
            let _ = req_tx.send(PreviewRequest {
                id: tree.id,
                tree: tree.clone(),
            });
            pending = Some(tree.id);
        }

        let preview = render_preview(selected, &git_cache);
        terminal
            .draw(|frame| view::draw(frame, app, &preview))
            .context("drawing the picker")?;

        let poll_ms = if pending.is_some() { 50 } else { 200 };
        if !event::poll(Duration::from_millis(poll_ms)).context("polling for input")? {
            continue;
        }
        let Event::Key(key) = event::read().context("reading input")? else {
            continue;
        };
        // Some terminals report both a press and a release; a release would
        // otherwise be handled a second time as if it were another press.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match state::reduce(app, key) {
            state::Reaction::None => {}
            state::Reaction::Cancel => return Ok(None),
            state::Reaction::Submit(request) => return Ok(Some(request)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_to_latest_keeps_only_the_last_queued_request() {
        let (tx, rx) = mpsc::channel();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();
        let first = rx.recv().unwrap();
        assert_eq!(drain_to_latest(first, &rx), 3);
    }

    #[test]
    fn drain_to_latest_returns_the_only_item_when_nothing_else_is_queued() {
        let (tx, rx) = mpsc::channel();
        tx.send("only").unwrap();
        let first = rx.recv().unwrap();
        assert_eq!(drain_to_latest(first, &rx), "only");
    }

    #[test]
    fn needs_request_is_false_when_cached() {
        let id = Uuid::now_v7();
        let mut cache = HashMap::new();
        cache.insert(id, "text".to_string());
        assert!(!needs_request(id, &cache, None));
    }

    #[test]
    fn needs_request_is_false_when_pending() {
        let id = Uuid::now_v7();
        let cache = HashMap::new();
        assert!(!needs_request(id, &cache, Some(id)));
    }

    #[test]
    fn needs_request_is_true_when_neither_cached_nor_pending() {
        let id = Uuid::now_v7();
        let other = Uuid::now_v7();
        let cache = HashMap::new();
        assert!(needs_request(id, &cache, Some(other)));
        assert!(needs_request(id, &cache, None));
    }
}
