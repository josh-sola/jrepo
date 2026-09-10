//! The launch picker: a ratatui screen that runs whenever `wt go` has no
//! selector and produces a full launch request. `cmd_launch` continues from
//! that request exactly as if its fields came from CLI flags.

mod state;
mod view;

use std::io::{self, IsTerminal};
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

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut state::State,
    store: &store::Store,
    config: &config::Config,
) -> Result<Option<LaunchRequest>> {
    let mut preview_cache: Option<(Uuid, String)> = None;
    loop {
        let preview = selected_preview(app, store, config, &mut preview_cache);
        terminal
            .draw(|frame| view::draw(frame, app, preview))
            .context("drawing the picker")?;

        if !event::poll(Duration::from_millis(200)).context("polling for input")? {
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

/// Recomputes the preview only when the highlighted tree changes: it shells
/// out to git, which is too slow to redo on every keystroke.
fn selected_preview<'a>(
    app: &state::State,
    store: &store::Store,
    config: &config::Config,
    cache: &'a mut Option<(Uuid, String)>,
) -> &'a str {
    let Some(&idx) = app.filtered.get(app.selected) else {
        *cache = None;
        return "";
    };
    let row = &app.rows[idx];
    if cache.as_ref().map(|(id, _)| *id) != Some(row.id) {
        let text = store
            .trees
            .iter()
            .find(|t| t.id == row.id)
            .map(|t| crate::launch_preview_lines(config, t).join("\n"))
            .unwrap_or_default();
        *cache = Some((row.id, text));
    }
    cache.as_ref().map(|(_, text)| text.as_str()).unwrap_or("")
}
