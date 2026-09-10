use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::agent::Agent;

use super::state::{Focus, FormFocus, Mode, NewTreeForm, Row, State, column_widths};

const HINT: &str = "type to filter  ·  ↑↓ move  ·  Tab focus  ·  Shift-Tab Pi/Claude  ·  Ctrl-P/L/X agent  ·  Enter launch  ·  Esc cancel";
const FORM_HINT: &str =
    "Tab next field  ·  Shift-Tab Pi/Claude  ·  ↑↓ pick repo  ·  Enter create  ·  Esc back to list";

pub fn draw(frame: &mut Frame, state: &State, preview: &str) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(6),
        ])
        .split(area);

    draw_filter(frame, rows[0], state);
    match &state.mode {
        Mode::List => draw_list_and_preview(frame, rows[1], state, preview),
        Mode::Form(form) => draw_form(frame, rows[1], form, &state.repos),
    }
    draw_launch_bar(frame, rows[2], state);
}

fn draw_filter(frame: &mut Frame, area: Rect, state: &State) {
    let dimmed = state.filter.starts_with('@');
    let title = if dimmed {
        "Filter (scratch selector)"
    } else {
        "Filter"
    };
    let style = if matches!(state.mode, Mode::List) && state.focus == Focus::Filter {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(style);
    frame.render_widget(Paragraph::new(state.filter.as_str()).block(block), area);
}

fn draw_list_and_preview(frame: &mut Frame, area: Rect, state: &State, preview: &str) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    draw_list(frame, cols[0], state);
    draw_preview(frame, cols[1], state, preview);
}

fn draw_list(frame: &mut Frame, area: Rect, state: &State) {
    let widths = column_widths(&state.rows);
    let dimmed = state.filter.starts_with('@');

    let header = format!(
        "{:<name$} {:<repo$} {:<branch$} {:<state$} AGE",
        "NAME",
        "REPO",
        "BRANCH",
        "STATE",
        name = widths.name,
        repo = widths.repo,
        branch = widths.branch,
        state = widths.state,
    );

    let rows_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    let header_style = if dimmed {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    frame.render_widget(Paragraph::new(header).style(header_style), rows_area[0]);

    let mut item_style = Style::default();
    if dimmed {
        item_style = item_style.add_modifier(Modifier::DIM);
    }
    let items: Vec<ListItem> = state
        .filtered
        .iter()
        .map(|&i| ListItem::new(list_line(&state.rows[i], &widths)).style(item_style))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut list_state = ListState::default();
    if !state.filtered.is_empty() {
        list_state.select(Some(state.selected));
    }
    frame.render_stateful_widget(list, rows_area[1], &mut list_state);
}

fn list_line(row: &Row, widths: &super::state::Widths) -> String {
    format!(
        "{:<name$} {:<repo$} {:<branch$} {:<state$} {}",
        row.name,
        row.repo,
        row.branch,
        row.state,
        crate::format_duration(row.age_secs),
        name = widths.name,
        repo = widths.repo,
        branch = widths.branch,
        state = widths.state,
    )
}

fn draw_preview(frame: &mut Frame, area: Rect, state: &State, preview: &str) {
    let block = Block::default().title("Preview").borders(Borders::ALL);
    let text = if let Some(label) = state.filter.strip_prefix('@') {
        if label.is_empty() {
            "type a name after '@' for a scratch session".to_string()
        } else {
            format!("Enter opens a scratch session named '@{label}'")
        }
    } else if preview.is_empty() {
        "no tree selected".to_string()
    } else {
        preview.to_string()
    };
    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn draw_form(frame: &mut Frame, area: Rect, form: &NewTreeForm, repos: &[String]) {
    let block = Block::default().title("New tree").borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    let repo_label = repos
        .get(form.repo_idx)
        .map(String::as_str)
        .unwrap_or("(no repos registered)");

    frame.render_widget(
        form_field("Name", &form.name, form.focus == FormFocus::Name),
        rows[0],
    );
    frame.render_widget(
        form_field("Repo", repo_label, form.focus == FormFocus::Repo),
        rows[1],
    );
    frame.render_widget(
        form_field("Branch", &form.branch, form.focus == FormFocus::Branch),
        rows[2],
    );
    frame.render_widget(
        form_field("Onto", &form.onto, form.focus == FormFocus::Onto),
        rows[3],
    );
    frame.render_widget(Paragraph::new(FORM_HINT), rows[4]);
}

fn form_field<'a>(label: &'a str, value: &'a str, focused: bool) -> Paragraph<'a> {
    let label_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let line = Line::from(vec![
        Span::styled(format!("{label:<7}"), label_style),
        Span::raw(value),
    ]);
    Paragraph::new(Text::from(line))
}

fn draw_launch_bar(frame: &mut Frame, area: Rect, state: &State) {
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(agent_line(state.agent)), rows[0]);

    let profile_focused = matches!(state.mode, Mode::List) && state.focus == Focus::Profile;
    frame.render_widget(
        field_line("Profile", &state.profile, profile_focused),
        rows[1],
    );

    let args_focused = matches!(state.mode, Mode::List) && state.focus == Focus::Args;
    frame.render_widget(field_line("Args", &state.args, args_focused), rows[2]);

    let hint = if matches!(state.mode, Mode::Form(_)) {
        FORM_HINT
    } else {
        HINT
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().add_modifier(Modifier::DIM)),
        rows[3],
    );
}

fn agent_line(agent: Agent) -> Line<'static> {
    let mut spans = vec![Span::raw("Agent  ")];
    for candidate in [Agent::Pi, Agent::Claude, Agent::Codex] {
        let label = agent_label(candidate);
        if candidate == agent {
            spans.push(Span::styled(
                format!("[{label}] "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw(format!("{label} ")));
        }
    }
    Line::from(spans)
}

fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::Pi => "Pi",
        Agent::Claude => "Claude",
        Agent::Codex => "Codex",
    }
}

fn field_line<'a>(label: &'a str, value: &'a str, focused: bool) -> Paragraph<'a> {
    let label_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let line = Line::from(vec![
        Span::styled(format!("{label}: "), label_style),
        Span::raw(value),
    ]);
    Paragraph::new(Text::from(line))
}
