//! Pure state for the launch picker: no terminal, no I/O. `reduce` maps one
//! key event to a new state plus a `Reaction`, so the whole interaction model
//! is unit-testable without a pty.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32Str};
use uuid::Uuid;

use crate::agent::Agent;
use crate::store;

use super::{LaunchRequest, Seed};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Focus {
    Filter,
    Profile,
    Args,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFocus {
    Name,
    Repo,
    Branch,
    Onto,
}

/// One row in the tree list. Built once when the picker opens and never
/// refreshed, so a tree's state or age can't shift the row out from under
/// the selection cursor mid-session.
#[derive(Debug, Clone)]
pub struct Row {
    pub id: Uuid,
    pub repo: String,
    pub name: String,
    pub branch: String,
    pub state: String,
    pub age_secs: i64,
    haystack: String,
}

#[derive(Debug, Clone)]
pub struct NewTreeForm {
    pub name: String,
    pub repo_idx: usize,
    pub branch: String,
    pub onto: String,
    pub focus: FormFocus,
}

#[derive(Debug, Clone)]
pub enum Mode {
    List,
    Form(NewTreeForm),
}

pub enum Reaction {
    None,
    Cancel,
    Submit(LaunchRequest),
}

pub struct State {
    pub rows: Vec<Row>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub filter: String,
    pub agent: Agent,
    pub profile: String,
    pub args: String,
    pub focus: Focus,
    pub mode: Mode,
    /// Carried through from `--repo` untouched; there is no field to edit it
    /// on the main screen, only in the new-tree form.
    pub repo: Option<String>,
    pub repos: Vec<String>,
    /// Used only to preselect a repo in the new-tree form when `--repo`
    /// didn't already seed one.
    cwd_repo: Option<String>,
    seed_branch: Option<String>,
    seed_onto: Option<String>,
}

impl State {
    pub fn new(rows: Vec<Row>, repos: Vec<String>, cwd_repo: Option<&str>, seed: Seed) -> Self {
        let filtered = (0..rows.len()).collect();
        let profile = seed.profile.map(|p| p.join(",")).unwrap_or_default();
        let args = join_args(&seed.args);
        State {
            rows,
            filtered,
            selected: 0,
            filter: String::new(),
            agent: seed.agent,
            profile,
            args,
            focus: Focus::Filter,
            mode: Mode::List,
            repo: seed.repo,
            repos,
            cwd_repo: cwd_repo.map(str::to_string),
            seed_branch: seed.branch,
            seed_onto: seed.onto,
        }
    }

    fn selected_row(&self) -> Option<&Row> {
        self.filtered.get(self.selected).map(|&i| &self.rows[i])
    }

    fn recompute_filter(&mut self) {
        self.filtered = matching_rows(&self.rows, &self.filter);
        self.selected = 0;
    }

    /// Matches clap's `value_delimiter = ','` shape for `--profile`, plus
    /// trimming: the field is hand-typed, unlike the flag.
    fn profile_field(&self) -> Option<Vec<String>> {
        let profiles: Vec<String> = self
            .profile
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (!profiles.is_empty()).then_some(profiles)
    }
}

/// The tree you want next is usually in the repo you are already sitting
/// in, or the one you just made, so those sort to the top.
pub fn order_trees<'a>(trees: &'a [store::Tree], cwd_repo: Option<&str>) -> Vec<&'a store::Tree> {
    let mut sorted: Vec<&store::Tree> = trees.iter().filter(|t| !t.spare).collect();
    sorted.sort_by(|a, b| {
        let a_home = cwd_repo.is_some_and(|r| a.repo == r);
        let b_home = cwd_repo.is_some_and(|r| b.repo == r);
        b_home.cmp(&a_home).then_with(|| b.created.cmp(&a.created))
    });
    sorted
}

pub fn build_rows(trees: &[store::Tree], cwd_repo: Option<&str>) -> Vec<Row> {
    let now = Utc::now();
    order_trees(trees, cwd_repo)
        .into_iter()
        .map(|t| row_from_tree(t, now))
        .collect()
}

fn row_from_tree(t: &store::Tree, now: DateTime<Utc>) -> Row {
    Row {
        id: t.id,
        repo: t.repo.clone(),
        name: t.name.clone(),
        branch: t.branch.clone(),
        state: crate::status_state_str(t),
        age_secs: (now - t.created).num_seconds(),
        haystack: format!("{} {} {}", t.repo, t.name, t.branch),
    }
}

/// Column widths for the list, computed once against the full row set so the
/// header lines up with rows the filter later hides.
pub struct Widths {
    pub name: usize,
    pub repo: usize,
    pub branch: usize,
    pub state: usize,
}

pub fn column_widths(rows: &[Row]) -> Widths {
    let w = |header: usize, vals: &mut dyn Iterator<Item = usize>| {
        vals.chain(std::iter::once(header)).max().unwrap_or(header)
    };
    Widths {
        name: w(4, &mut rows.iter().map(|r| r.name.chars().count())),
        repo: w(4, &mut rows.iter().map(|r| r.repo.chars().count())),
        branch: w(6, &mut rows.iter().map(|r| r.branch.chars().count())),
        state: w(5, &mut rows.iter().map(|r| r.state.len())),
    }
}

/// Keeps every row whose "repo name branch" haystack scores a fuzzy match,
/// in the original (grouped, newest-first) order — this narrows the list,
/// it never re-ranks it.
pub fn matching_rows(rows: &[Row], filter: &str) -> Vec<usize> {
    if filter.trim().is_empty() {
        return (0..rows.len()).collect();
    }
    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let pattern = Pattern::parse(filter, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    rows.iter()
        .enumerate()
        .filter(|(_, row)| {
            buf.clear();
            let haystack = Utf32Str::new(&row.haystack, &mut buf);
            pattern.score(haystack, &mut matcher).is_some()
        })
        .map(|(i, _)| i)
        .collect()
}

/// Splits passthrough args the way a shell would for a single quoted word:
/// whitespace separates arguments, and single or double quotes group one
/// argument together without the quote characters themselves.
pub fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut has_current = false;
    let mut in_single = false;
    let mut in_double = false;

    for c in input.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                has_current = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_current = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if has_current {
                    args.push(std::mem::take(&mut current));
                    has_current = false;
                }
            }
            c => {
                current.push(c);
                has_current = true;
            }
        }
    }
    if has_current {
        args.push(current);
    }
    args
}

fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The list view special-cases a leading `@`: the tree list is dimmed and
/// Enter submits the typed text directly as a scratch selector, instead of
/// matching against a tree.
fn is_direct_selector(filter: &str) -> bool {
    filter.starts_with('@')
}

pub fn reduce(state: &mut State, key: KeyEvent) -> Reaction {
    match &mut state.mode {
        Mode::List => reduce_list(state, key),
        Mode::Form(_) => reduce_form(state, key),
    }
}

fn agent_from_key(key: &KeyEvent) -> Option<Agent> {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    match key.code {
        KeyCode::Char('p') => Some(Agent::Pi),
        KeyCode::Char('l') => Some(Agent::Claude),
        KeyCode::Char('x') => Some(Agent::Codex),
        _ => None,
    }
}

// Codex has no dedicated Shift-Tab slot; it folds to Pi rather than Claude
// so the toggle stays a clean two-way flip once it's been reached from Codex.
fn toggle_pi_claude(agent: Agent) -> Agent {
    match agent {
        Agent::Pi => Agent::Claude,
        Agent::Claude => Agent::Pi,
        Agent::Codex => Agent::Pi,
    }
}

fn reduce_list(state: &mut State, key: KeyEvent) -> Reaction {
    if let Some(agent) = agent_from_key(&key) {
        state.agent = agent;
        return Reaction::None;
    }

    match key.code {
        KeyCode::Esc => return Reaction::Cancel,
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            return Reaction::None;
        }
        KeyCode::Down => {
            if state.selected + 1 < state.filtered.len() {
                state.selected += 1;
            }
            return Reaction::None;
        }
        KeyCode::Tab => {
            state.focus = match state.focus {
                Focus::Filter => Focus::Profile,
                Focus::Profile => Focus::Args,
                Focus::Args => Focus::Filter,
            };
            return Reaction::None;
        }
        KeyCode::BackTab => {
            state.agent = toggle_pi_claude(state.agent);
            return Reaction::None;
        }
        KeyCode::Enter => return submit_list(state),
        KeyCode::Backspace => {
            field_mut(state).pop();
            if state.focus == Focus::Filter {
                state.recompute_filter();
            }
            return Reaction::None;
        }
        KeyCode::Char(c) => {
            field_mut(state).push(c);
            if state.focus == Focus::Filter {
                state.recompute_filter();
            }
            return Reaction::None;
        }
        _ => {}
    }
    Reaction::None
}

fn field_mut(state: &mut State) -> &mut String {
    match state.focus {
        Focus::Filter => &mut state.filter,
        Focus::Profile => &mut state.profile,
        Focus::Args => &mut state.args,
    }
}

fn submit_list(state: &mut State) -> Reaction {
    if is_direct_selector(&state.filter) {
        return Reaction::Submit(LaunchRequest {
            selector: state.filter.clone(),
            repo: state.repo.clone(),
            branch: None,
            onto: None,
            profile: state.profile_field(),
            agent: state.agent,
            args: split_args(&state.args),
        });
    }

    if let Some(row) = state.selected_row() {
        return Reaction::Submit(LaunchRequest {
            selector: row.id.to_string(),
            repo: None,
            branch: None,
            onto: None,
            profile: state.profile_field(),
            agent: state.agent,
            args: split_args(&state.args),
        });
    }

    if state.filter.trim().is_empty() {
        return Reaction::None;
    }

    let repo_idx = state
        .repo
        .as_deref()
        .or(state.cwd_repo.as_deref())
        .and_then(|r| state.repos.iter().position(|x| x == r))
        .unwrap_or(0);
    state.mode = Mode::Form(NewTreeForm {
        name: state.filter.clone(),
        repo_idx,
        branch: state.seed_branch.clone().unwrap_or_default(),
        onto: state.seed_onto.clone().unwrap_or_default(),
        focus: FormFocus::Name,
    });
    Reaction::None
}

fn reduce_form(state: &mut State, key: KeyEvent) -> Reaction {
    if let Some(agent) = agent_from_key(&key) {
        state.agent = agent;
        return Reaction::None;
    }

    let Mode::Form(form) = &mut state.mode else {
        unreachable!("reduce_form only runs in Mode::Form");
    };

    match key.code {
        KeyCode::Esc => {
            state.mode = Mode::List;
            return Reaction::None;
        }
        KeyCode::Tab => {
            form.focus = next_form_focus(form.focus);
            return Reaction::None;
        }
        KeyCode::BackTab => {
            state.agent = toggle_pi_claude(state.agent);
            return Reaction::None;
        }
        KeyCode::Up if form.focus == FormFocus::Repo => {
            form.repo_idx = form.repo_idx.saturating_sub(1);
            return Reaction::None;
        }
        KeyCode::Down if form.focus == FormFocus::Repo => {
            if form.repo_idx + 1 < state.repos.len() {
                form.repo_idx += 1;
            }
            return Reaction::None;
        }
        KeyCode::Enter => return submit_form(state),
        KeyCode::Backspace => {
            if let Some(field) = form_field_mut(form) {
                field.pop();
            }
            return Reaction::None;
        }
        KeyCode::Char(c) => {
            if let Some(field) = form_field_mut(form) {
                field.push(c);
            }
            return Reaction::None;
        }
        _ => {}
    }
    Reaction::None
}

fn form_field_mut(form: &mut NewTreeForm) -> Option<&mut String> {
    match form.focus {
        FormFocus::Name => Some(&mut form.name),
        FormFocus::Branch => Some(&mut form.branch),
        FormFocus::Onto => Some(&mut form.onto),
        FormFocus::Repo => None,
    }
}

fn next_form_focus(focus: FormFocus) -> FormFocus {
    match focus {
        FormFocus::Name => FormFocus::Repo,
        FormFocus::Repo => FormFocus::Branch,
        FormFocus::Branch => FormFocus::Onto,
        FormFocus::Onto => FormFocus::Name,
    }
}

fn submit_form(state: &mut State) -> Reaction {
    let Mode::Form(form) = &state.mode else {
        unreachable!("submit_form only runs in Mode::Form");
    };
    if form.name.trim().is_empty() || state.repos.is_empty() {
        return Reaction::None;
    }
    let repo = state.repos[form.repo_idx].clone();
    let branch = (!form.branch.trim().is_empty()).then(|| form.branch.clone());
    let onto = (!form.onto.trim().is_empty()).then(|| form.onto.clone());
    Reaction::Submit(LaunchRequest {
        selector: form.name.clone(),
        repo: Some(repo),
        branch,
        onto,
        profile: state.profile_field(),
        agent: state.agent,
        args: split_args(&state.args),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
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
            spare: false,
        }
    }

    fn seed() -> Seed {
        Seed {
            repo: None,
            branch: None,
            onto: None,
            profile: None,
            agent: Agent::Pi,
            args: Vec::new(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn state_with(rows: Vec<Row>) -> State {
        State::new(
            rows,
            vec!["home".to_string(), "other".to_string()],
            Some("home"),
            seed(),
        )
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

        let names: Vec<&str> = order_trees(&trees, Some("home"))
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

        let names: Vec<&str> = order_trees(&trees, None)
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["newest", "middle", "oldest"]);
    }

    #[test]
    fn ordered_skips_unclaimed_spares() {
        let mut spare = tree_at("home", "spare", "b1", 1);
        spare.spare = true;
        let real = tree_at("home", "real", "b2", 2);
        let trees = vec![spare, real.clone()];

        let names: Vec<&str> = order_trees(&trees, None)
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["real"]);
    }

    fn rows_for(names: &[&str]) -> Vec<Row> {
        names
            .iter()
            .map(|n| Row {
                id: Uuid::now_v7(),
                repo: "mono".to_string(),
                name: n.to_string(),
                branch: format!("josh/{n}"),
                state: "ready".to_string(),
                age_secs: 0,
                haystack: format!("mono {n} josh/{n}"),
            })
            .collect()
    }

    #[test]
    fn fuzzy_filter_narrows_and_preserves_group_order() {
        let rows = rows_for(&["fix login", "fix logout", "add banner"]);
        let matched = matching_rows(&rows, "fix");
        let names: Vec<&str> = matched.iter().map(|&i| rows[i].name.as_str()).collect();
        assert_eq!(names, vec!["fix login", "fix logout"]);
    }

    #[test]
    fn fuzzy_filter_matches_across_repo_name_and_branch() {
        let rows = rows_for(&["alpha", "beta"]);
        let matched = matching_rows(&rows, "mono");
        assert_eq!(matched.len(), 2, "repo name should be searchable too");
    }

    #[test]
    fn empty_filter_keeps_every_row_in_order() {
        let rows = rows_for(&["a", "b", "c"]);
        assert_eq!(matching_rows(&rows, ""), vec![0, 1, 2]);
    }

    #[test]
    fn up_and_down_are_bounded_by_the_filtered_list() {
        let mut state = state_with(rows_for(&["a", "b"]));
        reduce(&mut state, key(KeyCode::Up));
        assert_eq!(state.selected, 0, "cannot go above the first row");
        reduce(&mut state, key(KeyCode::Down));
        assert_eq!(state.selected, 1);
        reduce(&mut state, key(KeyCode::Down));
        assert_eq!(state.selected, 1, "cannot go below the last row");
    }

    #[test]
    fn tab_cycles_filter_profile_args_and_wraps() {
        let mut state = state_with(rows_for(&["a"]));
        assert_eq!(state.focus, Focus::Filter);
        reduce(&mut state, key(KeyCode::Tab));
        assert_eq!(state.focus, Focus::Profile);
        reduce(&mut state, key(KeyCode::Tab));
        assert_eq!(state.focus, Focus::Args);
        reduce(&mut state, key(KeyCode::Tab));
        assert_eq!(state.focus, Focus::Filter);
    }

    #[test]
    fn shift_tab_toggles_pi_and_claude_in_the_list() {
        let mut state = state_with(rows_for(&["a"]));
        assert_eq!(state.agent, Agent::Pi);
        reduce(&mut state, key(KeyCode::BackTab));
        assert_eq!(state.agent, Agent::Claude);
        reduce(&mut state, key(KeyCode::BackTab));
        assert_eq!(state.agent, Agent::Pi);
    }

    #[test]
    fn shift_tab_from_codex_falls_to_pi() {
        let mut state = state_with(rows_for(&["a"]));
        reduce(&mut state, ctrl('x'));
        assert_eq!(state.agent, Agent::Codex);
        reduce(&mut state, key(KeyCode::BackTab));
        assert_eq!(state.agent, Agent::Pi);
    }

    #[test]
    fn typing_edits_the_focused_field_only() {
        let mut state = state_with(rows_for(&["fix login"]));
        reduce(&mut state, key(KeyCode::Tab));
        reduce(&mut state, key(KeyCode::Char('p')));
        assert_eq!(state.profile, "p");
        assert_eq!(state.filter, "");
    }

    #[test]
    fn profile_field_splits_trims_and_drops_empty_segments() {
        let mut state = state_with(rows_for(&["a"]));
        state.profile = " build, , test ".to_string();
        assert_eq!(
            state.profile_field(),
            Some(vec!["build".to_string(), "test".to_string()])
        );

        state.profile = "   ".to_string();
        assert_eq!(state.profile_field(), None);
    }

    #[test]
    fn ctrl_hotkeys_select_each_agent() {
        let mut state = state_with(rows_for(&["a"]));
        reduce(&mut state, ctrl('l'));
        assert_eq!(state.agent, Agent::Claude);
        reduce(&mut state, ctrl('x'));
        assert_eq!(state.agent, Agent::Codex);
        reduce(&mut state, ctrl('p'));
        assert_eq!(state.agent, Agent::Pi);
    }

    #[test]
    fn at_prefix_submits_directly_as_a_scratch_selector() {
        let mut state = state_with(rows_for(&["fix login"]));
        for c in "@poking-around".chars() {
            reduce(&mut state, key(KeyCode::Char(c)));
        }
        match reduce(&mut state, key(KeyCode::Enter)) {
            Reaction::Submit(req) => assert_eq!(req.selector, "@poking-around"),
            _ => panic!("expected a submit"),
        }
    }

    #[test]
    fn enter_on_a_match_submits_its_tree_id() {
        let rows = rows_for(&["fix login"]);
        let id = rows[0].id;
        let mut state = state_with(rows);
        for c in "fix".chars() {
            reduce(&mut state, key(KeyCode::Char(c)));
        }
        match reduce(&mut state, key(KeyCode::Enter)) {
            Reaction::Submit(req) => assert_eq!(req.selector, id.to_string()),
            _ => panic!("expected a submit"),
        }
    }

    #[test]
    fn enter_with_no_match_opens_the_form_with_the_name_prefilled() {
        let mut state = state_with(rows_for(&["fix login"]));
        for c in "brand new tree".chars() {
            reduce(&mut state, key(KeyCode::Char(c)));
        }
        reduce(&mut state, key(KeyCode::Enter));
        match &state.mode {
            Mode::Form(form) => assert_eq!(form.name, "brand new tree"),
            Mode::List => panic!("expected the new-tree form to open"),
        }
    }

    #[test]
    fn the_form_preselects_the_cwd_repo() {
        let mut state = state_with(rows_for(&["fix login"]));
        for c in "brand new tree".chars() {
            reduce(&mut state, key(KeyCode::Char(c)));
        }
        reduce(&mut state, key(KeyCode::Enter));
        match &state.mode {
            Mode::Form(form) => assert_eq!(state.repos[form.repo_idx], "home"),
            Mode::List => panic!("expected the new-tree form to open"),
        }
    }

    #[test]
    fn form_enter_yields_the_right_launch_request() {
        let mut state = state_with(rows_for(&["fix login"]));
        for c in "brand new".chars() {
            reduce(&mut state, key(KeyCode::Char(c)));
        }
        reduce(&mut state, key(KeyCode::Enter));
        reduce(&mut state, key(KeyCode::Tab)); // Name -> Repo
        reduce(&mut state, key(KeyCode::Down)); // home -> other
        reduce(&mut state, key(KeyCode::Tab)); // Repo -> Branch
        for c in "my-branch".chars() {
            reduce(&mut state, key(KeyCode::Char(c)));
        }
        match reduce(&mut state, key(KeyCode::Enter)) {
            Reaction::Submit(req) => {
                assert_eq!(req.selector, "brand new");
                assert_eq!(req.repo.as_deref(), Some("other"));
                assert_eq!(req.branch.as_deref(), Some("my-branch"));
                assert_eq!(req.onto, None);
            }
            _ => panic!("expected a submit"),
        }
    }

    #[test]
    fn shift_tab_in_the_form_toggles_agent_not_focus() {
        let mut state = state_with(rows_for(&["fix login"]));
        for c in "brand new tree".chars() {
            reduce(&mut state, key(KeyCode::Char(c)));
        }
        reduce(&mut state, key(KeyCode::Enter));
        reduce(&mut state, key(KeyCode::Tab)); // Name -> Repo
        assert_eq!(state.agent, Agent::Pi);

        reduce(&mut state, key(KeyCode::BackTab));
        assert_eq!(state.agent, Agent::Claude);
        match &state.mode {
            Mode::Form(form) => assert_eq!(form.focus, FormFocus::Repo, "focus is untouched"),
            Mode::List => panic!("expected to still be in the form"),
        }
    }

    #[test]
    fn esc_from_the_form_returns_to_the_list() {
        let mut state = state_with(rows_for(&["fix login"]));
        for c in "brand new tree".chars() {
            reduce(&mut state, key(KeyCode::Char(c)));
        }
        reduce(&mut state, key(KeyCode::Enter));
        assert!(matches!(state.mode, Mode::Form(_)));
        reduce(&mut state, key(KeyCode::Esc));
        assert!(matches!(state.mode, Mode::List));
        assert_eq!(state.filter, "brand new tree", "the list keeps its filter");
    }

    #[test]
    fn esc_from_the_list_cancels() {
        let mut state = state_with(rows_for(&["a"]));
        assert!(matches!(
            reduce(&mut state, key(KeyCode::Esc)),
            Reaction::Cancel
        ));
    }

    #[test]
    fn split_args_honors_single_and_double_quotes() {
        assert_eq!(
            split_args(r#"--model gpt-5 -n "fix login" 'a b'"#),
            vec!["--model", "gpt-5", "-n", "fix login", "a b"]
        );
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args("  a   b  "), vec!["a", "b"]);
    }
}
