pub mod kitty;
pub mod raster;
pub mod widgets;

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyCode, KeyEvent,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use miette::IntoDiagnostic;
use notify::{RecursiveMode, Watcher};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs as RatatuiTabs, Widget, Wrap};
use ratatui::{Frame, Terminal};

use crate::model::{LogLevel, Node, SplitDirection, StateMap, Status, infer_lang_from_path};
use crate::render::kitty::{
    CellPixelSize, KittyPlacement, KittyPresenter, detect_kitty_support, query_cell_pixel_size,
    render_half_blocks, render_half_blocks_cropped,
};
use crate::render::raster::{RasterCache, RasterCacheKey};
use crate::render::widgets::code::{render_code_lines, render_code_lines_wrapped};
use crate::render::widgets::markdown::render_markdown_lines;
use crate::render::widgets::{
    LogRenderArgs, TableRenderArgs, callout_block, pane_block, render_log_into_buffer,
    render_stat_into_buffer, render_table_into_buffer, render_tooltip_overlay, to_ratatui_color,
};
use crate::session::read_frames_from;
use crate::store::{Store, compute_chart_scales, resolve_code_file};
use crate::svg::chart::{ChartRenderInput, render_chart_svg};
use crate::svg::graph::{GraphRenderInput, render_graph_svg};
use crate::svg::hit_test::prepare_custom_svg;
use crate::svg::{HitMap, HoverHit, SvgBundle};
use crate::theme::{Theme, ThemeMode, resolve_theme_mode};

struct GraphicPaneMeta<'a> {
    pane_id: &'a str,
    title: Option<&'a str>,
    caption: Option<&'a str>,
    focused: bool,
    content_hash: u64,
}

struct ScrollSlice {
    dest_rect: Rect,
    full_height: u16,
    clip_top: u16,
}

#[derive(Debug, Clone)]
pub struct ScrollViewport {
    pub id: String,
    pub rect: Rect,
    pub scroll: usize,
    pub total_height: usize,
    pub visible_height: usize,
}

#[derive(Debug, Clone)]
pub struct GraphicPaneHitArea {
    pub pane_id: String,
    pub inner_rect: Rect,
    pub hit_map: HitMap,
    pub px_w: u32,
    pub px_h: u32,
    pub px_y_offset: u32,
}

#[derive(Debug, Clone)]
pub struct TabHitArea {
    pub tabs_id: String,
    pub tab_index: usize,
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub struct TableHitArea {
    pub table_id: String,
    pub inner_rect: Rect,
    pub row_ids: Vec<Option<String>>,
    pub row_y_spans: Vec<(usize, u16, u16)>,
}

#[derive(Debug, Clone)]
pub struct PaneArea {
    pub key: String,
    pub rect: Rect,
    pub kind: PaneKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    Scroll,
    Text,
    Table,
    Log,
    Graphic,
    Other,
}

#[derive(Debug, Clone)]
pub struct ActiveHover {
    pub hit: HoverHit,
    pub cursor_col: u16,
    pub cursor_row: u16,
}

pub struct App {
    pub store: Store,
    pub theme: Theme,
    pub cache: RasterCache,
    pub kitty_enabled: bool,
    pub cell_px: CellPixelSize,
    pub code_wrap: bool,
    pub zoomed_pane: Option<String>,
    pub focused_pane: usize,
    pub focus_order: Vec<String>,
    pub tabs_order: Vec<(String, usize)>,
    pub scrolls: BTreeMap<String, usize>,
    pub h_scrolls: BTreeMap<String, usize>,
    pub scroll_viewports: Vec<ScrollViewport>,
    pub log_follow: BTreeMap<String, bool>,
    pub log_counts: BTreeMap<String, usize>,
    pub text_lengths: BTreeMap<String, usize>,
    pub pane_heights: BTreeMap<String, usize>,
    pub table_cursor: BTreeMap<String, usize>,
    pub table_lengths: BTreeMap<String, usize>,
    pub table_row_ids: BTreeMap<String, Vec<Option<String>>>,
    pub last_code_focus: BTreeMap<String, Option<(usize, usize)>>,
    pub log_min_level: Option<LogLevel>,
    pub search_mode: bool,
    pub search_query: String,
    pub hover: Option<ActiveHover>,
    pub should_quit: bool,
    pub graphic_areas: Vec<GraphicPaneHitArea>,
    pub tab_areas: Vec<TabHitArea>,
    pub table_areas: Vec<TableHitArea>,
    pub pane_areas: Vec<PaneArea>,
    pub pending_kitty: Vec<KittyPlacement>,
    chart_scales: BTreeMap<String, f64>,
}

impl App {
    pub fn new(store: Store, theme: Theme, kitty_enabled: bool, cell_px: CellPixelSize) -> Self {
        Self {
            store,
            theme,
            cache: RasterCache::default(),
            kitty_enabled,
            cell_px,
            code_wrap: true,
            zoomed_pane: None,
            focused_pane: 0,
            focus_order: Vec::new(),
            tabs_order: Vec::new(),
            scrolls: BTreeMap::new(),
            h_scrolls: BTreeMap::new(),
            scroll_viewports: Vec::new(),
            log_follow: BTreeMap::new(),
            log_counts: BTreeMap::new(),
            text_lengths: BTreeMap::new(),
            pane_heights: BTreeMap::new(),
            table_cursor: BTreeMap::new(),
            table_lengths: BTreeMap::new(),
            table_row_ids: BTreeMap::new(),
            last_code_focus: BTreeMap::new(),
            log_min_level: None,
            search_mode: false,
            search_query: String::new(),
            hover: None,
            should_quit: false,
            graphic_areas: Vec::new(),
            tab_areas: Vec::new(),
            table_areas: Vec::new(),
            pane_areas: Vec::new(),
            pending_kitty: Vec::new(),
            chart_scales: BTreeMap::new(),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        self.focus_order.clear();
        self.tabs_order.clear();
        self.scroll_viewports.clear();
        self.graphic_areas.clear();
        self.tab_areas.clear();
        self.table_areas.clear();
        self.pane_areas.clear();
        self.pending_kitty.clear();

        let area = frame.area();
        let bg_style = Style::default()
            .bg(to_ratatui_color(self.theme.bg))
            .fg(to_ratatui_color(self.theme.fg));
        frame.render_widget(Block::default().style(bg_style), area);

        let has_steps = !self.store.doc.steps.is_empty();
        let step_bar_h = if has_steps {
            let inner_w = area.width.saturating_sub(2).max(1);
            let wrapped_lines = self
                .store
                .doc
                .steps
                .get(self.store.step_index)
                .map(|s| {
                    let lines = render_markdown_lines(&s.body, &self.theme);
                    wrapped_line_count(&lines, inner_w)
                })
                .unwrap_or(1);
            let max_step_h = (area.height / 3).max(4);
            (wrapped_lines + 2).clamp(3, max_step_h)
        } else {
            0
        };

        let mut constraints = vec![Constraint::Length(1)];
        if has_steps {
            constraints.push(Constraint::Length(step_bar_h));
        }
        constraints.push(Constraint::Min(3));
        constraints.push(Constraint::Length(1));

        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        let (main_area, footer_area) = if has_steps {
            self.draw_step_bar(frame, sections[1]);
            (sections[2], sections[3])
        } else {
            (sections[1], sections[2])
        };

        let effective = self.store.effective_state();
        let root = self.store.doc.root.clone();
        self.chart_scales = compute_chart_scales(&root);
        if let Some(zoom_key) = self.zoomed_pane.clone() {
            if let Some(zoomed) = find_node_by_key(&root, "root", &zoom_key).cloned() {
                self.draw_node(frame, &zoomed, main_area, &zoom_key, &effective);
            } else {
                self.zoomed_pane = None;
                self.draw_node(frame, &root, main_area, "root", &effective);
            }
        } else {
            self.draw_node(frame, &root, main_area, "root", &effective);
        }

        if !self.focus_order.is_empty() && self.focused_pane >= self.focus_order.len() {
            self.focused_pane = self.focus_order.len() - 1;
        }

        // Draw header and footer after main content so scroll position reflects the current layout.
        self.draw_header(frame, sections[0]);
        self.draw_footer(frame, footer_area);

        if let Some(hover) = &self.hover {
            render_tooltip_overlay(
                frame,
                &hover.hit,
                hover.cursor_col,
                hover.cursor_row,
                &self.theme,
            );
        }
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let mut right_spans: Vec<Span<'static>> = Vec::new();
        if self.zoomed_pane.is_some() {
            right_spans.push(Span::styled(
                " [ZOOM] ",
                Style::default()
                    .fg(to_ratatui_color(self.theme.bg))
                    .bg(to_ratatui_color(self.theme.accent))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if let Some(vp) = self.scroll_viewports.first() {
            let line_num = (vp.scroll + 1).min(vp.total_height.max(1));
            right_spans.push(Span::styled(
                format!(" {line_num}/{} ", vp.total_height.max(1)),
                Style::default()
                    .fg(to_ratatui_color(self.theme.fg))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if self.store.closed {
            right_spans.push(Span::styled(
                " [CLOSED] ",
                Style::default()
                    .fg(to_ratatui_color(self.theme.bg))
                    .bg(to_ratatui_color(self.theme.warn))
                    .add_modifier(Modifier::BOLD),
            ));
        }

        let total_w = usize::from(area.width);
        let right_w: usize = right_spans.iter().map(|s| s.content.chars().count()).sum();
        let avail_left = total_w.saturating_sub(right_w);

        let title_style = Style::default()
            .fg(to_ratatui_color(self.theme.accent))
            .add_modifier(Modifier::BOLD);
        let sub_style = Style::default().fg(to_ratatui_color(self.theme.muted));

        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut left_w = 0usize;
        if avail_left > 1 {
            let title_text = format!(" {}", self.store.doc.title);
            let title_len = title_text.chars().count();
            if title_len >= avail_left {
                let clipped = truncate_with_ellipsis(&title_text, avail_left.saturating_sub(1));
                left_w = clipped.chars().count();
                spans.push(Span::styled(clipped, title_style));
            } else {
                left_w = title_len;
                spans.push(Span::styled(title_text, title_style));
                if let Some(sub) = &self.store.doc.subtitle {
                    let rem = avail_left.saturating_sub(left_w + 1);
                    if rem > 4 {
                        let sub_text = format!(" — {sub}");
                        let clipped = truncate_with_ellipsis(&sub_text, rem);
                        left_w += clipped.chars().count();
                        spans.push(Span::styled(clipped, sub_style));
                    }
                }
            }
        }

        let pad = total_w.saturating_sub(left_w + right_w);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.extend(right_spans);

        let header = Paragraph::new(Line::from(spans)).style(
            Style::default()
                .bg(to_ratatui_color(self.theme.surface_alt))
                .fg(to_ratatui_color(self.theme.fg)),
        );
        frame.render_widget(header, area);
    }

    fn draw_step_bar(&self, frame: &mut Frame<'_>, area: Rect) {
        let total = self.store.doc.steps.len();
        let Some(step) = self.store.doc.steps.get(self.store.step_index) else {
            return;
        };
        let title = format!(" [{}/{}] {} ", self.store.step_index + 1, total, step.title);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(to_ratatui_color(self.theme.accent)))
            .style(Style::default().bg(to_ratatui_color(self.theme.surface)))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(to_ratatui_color(self.theme.accent))
                    .add_modifier(Modifier::BOLD),
            ));
        let lines = render_markdown_lines(&step.body, &self.theme);
        let para = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let content = if self.search_mode {
            Line::from(vec![
                Span::styled(
                    " /",
                    Style::default()
                        .fg(to_ratatui_color(self.theme.accent))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{}▏", self.search_query),
                    Style::default().fg(to_ratatui_color(self.theme.fg)),
                ),
                Span::styled(
                    "  (Enter apply, Esc clear)",
                    Style::default().fg(to_ratatui_color(self.theme.muted)),
                ),
            ])
        } else {
            let mut hint = if !self.scroll_viewports.is_empty() {
                String::from(" q quit · j/k scroll · J/K page · g/G end · w wrap · t theme")
            } else if !self.store.doc.steps.is_empty() {
                String::from(" q quit · ←/→ step · Tab focus · z zoom · w wrap · t theme")
            } else {
                String::from(" q quit · Tab focus · z zoom · w wrap · [/] tabs · t theme")
            };
            if let Some(vp) = self.scroll_viewports.first() {
                let line_num = (vp.scroll + 1).min(vp.total_height.max(1));
                hint.push_str(&format!(" · {line_num}/{}", vp.total_height.max(1)));
            }
            if let Some(sel) = self
                .store
                .effective_state()
                .get("selected")
                .and_then(|v| v.as_str())
                && hint.chars().count() + sel.chars().count() + 8 <= usize::from(area.width)
            {
                hint.push_str(&format!(" · sel:{sel}"));
            }
            let fitted = truncate_with_ellipsis(&hint, usize::from(area.width));
            Line::from(Span::styled(
                fitted,
                Style::default().fg(to_ratatui_color(self.theme.muted)),
            ))
        };
        let footer = Paragraph::new(content)
            .style(Style::default().bg(to_ratatui_color(self.theme.surface_alt)));
        frame.render_widget(footer, area);
    }

    fn draw_node(
        &mut self,
        frame: &mut Frame<'_>,
        node: &Node,
        area: Rect,
        path: &str,
        state: &StateMap,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        match node {
            Node::Split {
                direction,
                ratio,
                weights,
                children,
            } => {
                if children.is_empty() {
                    return;
                }
                let constraints = split_constraints(*ratio, weights, children.len());
                let dir = match direction {
                    SplitDirection::Horizontal => {
                        let preview = Layout::default()
                            .direction(Direction::Horizontal)
                            .constraints(constraints.clone())
                            .split(area);
                        let min_col_w = preview.iter().map(|c| c.width).min().unwrap_or(0);
                        if min_col_w < 42
                            && area.height >= (children.len() as u16).saturating_mul(8)
                        {
                            Direction::Vertical
                        } else {
                            Direction::Horizontal
                        }
                    }
                    SplitDirection::Vertical => Direction::Vertical,
                };
                let chunks = Layout::default()
                    .direction(dir)
                    .constraints(constraints)
                    .split(area);
                for (idx, (child, chunk)) in children.iter().zip(chunks.iter()).enumerate() {
                    self.draw_node(frame, child, *chunk, &format!("{path}.{idx}"), state);
                }
            }
            Node::Scroll { id, children } => {
                self.draw_scroll(frame, id, children, area, path, state);
            }
            Node::Tabs { id, items } => {
                if items.is_empty() {
                    return;
                }
                self.tabs_order.push((id.clone(), items.len()));
                let active = self
                    .store
                    .active_tabs
                    .get(id)
                    .copied()
                    .unwrap_or(0)
                    .min(items.len() - 1);

                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(1), Constraint::Min(1)])
                    .split(area);

                let tab_bar = rows[0];
                let titles: Vec<Line<'static>> = items
                    .iter()
                    .map(|item| Line::from(format!(" {} ", item.title)))
                    .collect();

                let mut x_cursor = tab_bar.x;
                for (idx, item) in items.iter().enumerate() {
                    let w = (item.title.chars().count() as u16 + 2)
                        .min(tab_bar.right().saturating_sub(x_cursor));
                    if w > 0 {
                        self.tab_areas.push(TabHitArea {
                            tabs_id: id.clone(),
                            tab_index: idx,
                            rect: Rect::new(x_cursor, tab_bar.y, w, 1),
                        });
                    }
                    x_cursor = x_cursor.saturating_add(w + 1);
                }

                let tabs_widget = RatatuiTabs::new(titles)
                    .select(active)
                    .style(
                        Style::default()
                            .bg(to_ratatui_color(self.theme.surface_alt))
                            .fg(to_ratatui_color(self.theme.muted)),
                    )
                    .highlight_style(
                        Style::default()
                            .fg(to_ratatui_color(self.theme.accent))
                            .bg(to_ratatui_color(self.theme.selection))
                            .add_modifier(Modifier::BOLD),
                    )
                    .divider("│");
                frame.render_widget(tabs_widget, tab_bar);

                self.draw_node(
                    frame,
                    &items[active].node,
                    rows[1],
                    &format!("{path}.tab{active}"),
                    state,
                );
            }
            Node::Markdown {
                id,
                title,
                tone,
                content,
                by_selected,
                ..
            } => {
                let pane_key = id.clone().unwrap_or_else(|| path.to_string());
                let focused = self.register_focus(&pane_key, area, PaneKind::Text);
                let scroll = self.scrolls.get(&pane_key).copied().unwrap_or(0) as u16;
                let (inner_h, total_lines) = self.render_markdown_into_buffer(
                    frame.buffer_mut(),
                    area,
                    MarkdownRenderArgs {
                        title: title.as_deref(),
                        tone: *tone,
                        content,
                        by_selected,
                        borderless: false,
                        focused,
                        scroll,
                    },
                    state,
                );
                let max_scroll = total_lines.saturating_sub(usize::from(inner_h));
                if let Some(s) = self.scrolls.get_mut(&pane_key) {
                    *s = (*s).min(max_scroll);
                }
                self.text_lengths.insert(pane_key.clone(), total_lines);
                self.pane_heights.insert(pane_key, usize::from(inner_h));
            }
            Node::Code {
                id,
                title,
                lang,
                path: file_path,
                lines,
                source,
                start_line,
                highlight_lines,
                focus_key,
                by_selected,
                ..
            } => {
                let pane_key = id.clone().unwrap_or_else(|| path.to_string());
                let focused = self.register_focus(&pane_key, area, PaneKind::Text);
                let (inner_h, total_lines) = self.render_code_into_buffer(
                    frame.buffer_mut(),
                    area,
                    CodeRenderArgs {
                        pane_key: &pane_key,
                        title: title.as_deref(),
                        lang: lang.as_deref(),
                        path: file_path.as_deref(),
                        lines: *lines,
                        source,
                        start_line: *start_line,
                        highlight_lines,
                        focus_key: focus_key.as_deref(),
                        by_selected,
                        focused,
                    },
                    state,
                );
                self.text_lengths.insert(pane_key.clone(), total_lines);
                self.pane_heights.insert(pane_key, usize::from(inner_h));
            }
            Node::Stat {
                id, title, items, ..
            } => {
                let pane_key = id.clone().unwrap_or_else(|| path.to_string());
                let focused = self.register_focus(&pane_key, area, PaneKind::Other);
                render_stat_into_buffer(
                    frame.buffer_mut(),
                    area,
                    title.as_deref(),
                    items,
                    focused,
                    &self.theme,
                );
            }
            Node::Table {
                id,
                title,
                columns,
                rows,
                ..
            } => {
                let pane_key = id.clone().unwrap_or_else(|| path.to_string());
                let focused = self.register_focus(&pane_key, area, PaneKind::Table);
                let cursor = self
                    .table_cursor
                    .get(&pane_key)
                    .copied()
                    .unwrap_or(0)
                    .min(rows.len().saturating_sub(1));
                let scroll = self.scrolls.get(&pane_key).copied().unwrap_or(0);
                let selected_id = state.get("selected").and_then(|v| v.as_str());

                let tbl_out = render_table_into_buffer(
                    frame.buffer_mut(),
                    area,
                    &TableRenderArgs {
                        title: title.as_deref(),
                        columns,
                        rows,
                        selected_id,
                        cursor,
                        scroll,
                        focused,
                    },
                    &self.theme,
                );
                let row_ids: Vec<Option<String>> = rows.iter().map(|r| r.id.clone()).collect();
                self.table_lengths.insert(pane_key.clone(), rows.len());
                self.table_row_ids.insert(pane_key.clone(), row_ids.clone());
                self.pane_heights.insert(
                    pane_key.clone(),
                    usize::from(tbl_out.inner.height.saturating_sub(1)),
                );
                self.table_areas.push(TableHitArea {
                    table_id: pane_key,
                    inner_rect: tbl_out.inner,
                    row_ids,
                    row_y_spans: tbl_out.row_y_spans,
                });
            }
            Node::Log {
                id, title, entries, ..
            } => {
                let focused = self.register_focus(id, area, PaneKind::Log);
                let follow = *self.log_follow.get(id).unwrap_or(&true);
                let scroll = self.scrolls.get(id).copied().unwrap_or(0);
                let inner_h = area.height.saturating_sub(2);
                self.pane_heights.insert(id.clone(), usize::from(inner_h));
                let count = render_log_into_buffer(
                    frame.buffer_mut(),
                    area,
                    &LogRenderArgs {
                        title: title.as_deref(),
                        entries,
                        min_level: self.log_min_level,
                        search_query: &self.search_query,
                        scroll,
                        follow,
                        focused,
                    },
                    &self.theme,
                );
                self.log_counts.insert(id.clone(), count);
            }
            Node::Graph {
                id,
                title,
                caption,
                direction,
                nodes,
                edges,
                highlight_key,
                select_key,
                ..
            } => {
                let focused = self.register_focus(id, area, PaneKind::Graphic);
                let sel_key = select_key.as_deref().unwrap_or("selected");
                let hl_key = highlight_key.as_deref().unwrap_or("highlight");
                let selected = state.get(sel_key).and_then(|v| v.as_str());
                let highlighted = state.get(hl_key).map(|v| v.as_list()).unwrap_or_default();

                let content_hash = hash_render_key(&(
                    "graph",
                    id,
                    direction,
                    nodes,
                    edges,
                    selected,
                    &highlighted,
                ));
                let theme = self.theme;
                self.draw_graphic_pane(
                    frame,
                    ScrollSlice {
                        dest_rect: area,
                        full_height: area.height,
                        clip_top: 0,
                    },
                    GraphicPaneMeta {
                        pane_id: id,
                        title: title.as_deref(),
                        caption: caption.as_deref(),
                        focused,
                        content_hash,
                    },
                    |w, h| {
                        Ok(render_graph_svg(
                            &GraphRenderInput {
                                nodes,
                                edges,
                                direction: *direction,
                                selected,
                                highlighted: &highlighted,
                            },
                            &theme,
                            w,
                            h,
                        ))
                    },
                );
            }
            Node::Chart {
                id,
                title,
                caption,
                chart_type,
                x_labels,
                x_unit,
                y_unit,
                value_labels,
                scale,
                series,
                ..
            } => {
                let focused = self.register_focus(id, area, PaneKind::Graphic);
                let max_override = scale
                    .as_deref()
                    .and_then(|s| self.chart_scales.get(s).copied());
                let content_hash = hash_render_key(&(
                    "chart",
                    id,
                    title,
                    chart_type,
                    x_labels,
                    x_unit,
                    y_unit,
                    value_labels,
                    max_override.map(f64::to_bits),
                    series,
                ));
                let theme = self.theme;
                self.draw_graphic_pane(
                    frame,
                    ScrollSlice {
                        dest_rect: area,
                        full_height: area.height,
                        clip_top: 0,
                    },
                    GraphicPaneMeta {
                        pane_id: id,
                        title: title.as_deref(),
                        caption: caption.as_deref(),
                        focused,
                        content_hash,
                    },
                    |w, h| {
                        render_chart_svg(
                            &ChartRenderInput {
                                id,
                                title: None,
                                chart_type: *chart_type,
                                x_labels,
                                x_unit: x_unit.as_deref(),
                                y_unit: y_unit.as_deref(),
                                value_labels: *value_labels,
                                max_override,
                                series,
                            },
                            &theme,
                            w,
                            h,
                        )
                    },
                );
            }
            Node::Svg {
                id,
                title,
                caption,
                source,
                tooltips,
                select_key,
                highlight_key,
                ..
            } => {
                let focused = self.register_focus(id, area, PaneKind::Graphic);
                let sel_key = select_key.as_deref().unwrap_or("selected");
                let hl_key = highlight_key.as_deref().unwrap_or("highlight");
                let selected = state.get(sel_key).and_then(|v| v.as_str());
                let highlighted = state.get(hl_key).map(|v| v.as_list()).unwrap_or_default();

                let content_hash =
                    hash_render_key(&("svg", id, source, tooltips, selected, &highlighted));
                let theme = self.theme;
                self.draw_graphic_pane(
                    frame,
                    ScrollSlice {
                        dest_rect: area,
                        full_height: area.height,
                        clip_top: 0,
                    },
                    GraphicPaneMeta {
                        pane_id: id,
                        title: title.as_deref(),
                        caption: caption.as_deref(),
                        focused,
                        content_hash,
                    },
                    |w, h| {
                        prepare_custom_svg(source, tooltips, selected, &highlighted, &theme, w, h)
                    },
                );
            }
        }
    }

    fn draw_scroll(
        &mut self,
        frame: &mut Frame<'_>,
        id: &str,
        children: &[Node],
        area: Rect,
        path: &str,
        state: &StateMap,
    ) {
        self.register_focus(id, area, PaneKind::Scroll);

        let child_heights: Vec<u16> = children
            .iter()
            .map(|c| self.measure_node_height(c, area.width, state))
            .collect();

        let gap: usize = 1;
        let total_height: usize = child_heights.iter().map(|&h| usize::from(h)).sum::<usize>()
            + children.len().saturating_sub(1) * gap;
        let visible_height = usize::from(area.height);
        let max_scroll = total_height.saturating_sub(visible_height);
        let scroll = self.scrolls.get(id).copied().unwrap_or(0).min(max_scroll);
        self.scrolls.insert(id.to_string(), scroll);
        self.pane_heights.insert(id.to_string(), visible_height);
        self.scroll_viewports.push(ScrollViewport {
            id: id.to_string(),
            rect: area,
            scroll,
            total_height,
            visible_height,
        });

        let vp_start = scroll;
        let vp_end = scroll + visible_height;
        let mut cursor_y = 0usize;

        for (idx, (child, &child_h)) in children.iter().zip(child_heights.iter()).enumerate() {
            let child_start = cursor_y;
            let child_end = cursor_y + usize::from(child_h);
            cursor_y = child_end + gap;

            if child_end <= vp_start || child_start >= vp_end {
                continue;
            }

            let vis_start = child_start.max(vp_start);
            let vis_end = child_end.min(vp_end);
            let vis_h = (vis_end - vis_start) as u16;
            let clip_top = (vis_start - child_start) as u16;
            let dest_y = area.y + (vis_start - vp_start) as u16;
            let dest_rect = Rect::new(area.x, dest_y, area.width, vis_h);
            let child_path = format!("{path}.{idx}");

            self.draw_scroll_child(
                frame,
                child,
                ScrollSlice {
                    dest_rect,
                    full_height: child_h,
                    clip_top,
                },
                &child_path,
                state,
            );
        }
    }

    fn draw_scroll_child(
        &mut self,
        frame: &mut Frame<'_>,
        node: &Node,
        slice: ScrollSlice,
        path: &str,
        state: &StateMap,
    ) {
        let width = slice.dest_rect.width;
        if width == 0 || slice.dest_rect.height == 0 {
            return;
        }

        match node {
            Node::Graph {
                id,
                title,
                caption,
                direction,
                nodes,
                edges,
                highlight_key,
                select_key,
                ..
            } => {
                let sel_key = select_key.as_deref().unwrap_or("selected");
                let hl_key = highlight_key.as_deref().unwrap_or("highlight");
                let selected = state.get(sel_key).and_then(|v| v.as_str());
                let highlighted = state.get(hl_key).map(|v| v.as_list()).unwrap_or_default();
                let content_hash = hash_render_key(&(
                    "graph",
                    id,
                    direction,
                    nodes,
                    edges,
                    selected,
                    &highlighted,
                ));
                let theme = self.theme;
                self.draw_graphic_pane(
                    frame,
                    slice,
                    GraphicPaneMeta {
                        pane_id: id,
                        title: title.as_deref(),
                        caption: caption.as_deref(),
                        focused: false,
                        content_hash,
                    },
                    |w, h| {
                        Ok(render_graph_svg(
                            &GraphRenderInput {
                                nodes,
                                edges,
                                direction: *direction,
                                selected,
                                highlighted: &highlighted,
                            },
                            &theme,
                            w,
                            h,
                        ))
                    },
                );
            }
            Node::Chart {
                id,
                title,
                caption,
                chart_type,
                x_labels,
                x_unit,
                y_unit,
                value_labels,
                scale,
                series,
                ..
            } => {
                let max_override = scale
                    .as_deref()
                    .and_then(|s| self.chart_scales.get(s).copied());
                let content_hash = hash_render_key(&(
                    "chart",
                    id,
                    title,
                    chart_type,
                    x_labels,
                    x_unit,
                    y_unit,
                    value_labels,
                    max_override.map(f64::to_bits),
                    series,
                ));
                let theme = self.theme;
                self.draw_graphic_pane(
                    frame,
                    slice,
                    GraphicPaneMeta {
                        pane_id: id,
                        title: title.as_deref(),
                        caption: caption.as_deref(),
                        focused: false,
                        content_hash,
                    },
                    |w, h| {
                        render_chart_svg(
                            &ChartRenderInput {
                                id,
                                title: None,
                                chart_type: *chart_type,
                                x_labels,
                                x_unit: x_unit.as_deref(),
                                y_unit: y_unit.as_deref(),
                                value_labels: *value_labels,
                                max_override,
                                series,
                            },
                            &theme,
                            w,
                            h,
                        )
                    },
                );
            }
            Node::Svg {
                id,
                title,
                caption,
                source,
                tooltips,
                select_key,
                highlight_key,
                ..
            } => {
                let sel_key = select_key.as_deref().unwrap_or("selected");
                let hl_key = highlight_key.as_deref().unwrap_or("highlight");
                let selected = state.get(sel_key).and_then(|v| v.as_str());
                let highlighted = state.get(hl_key).map(|v| v.as_list()).unwrap_or_default();
                let content_hash =
                    hash_render_key(&("svg", id, source, tooltips, selected, &highlighted));
                let theme = self.theme;
                self.draw_graphic_pane(
                    frame,
                    slice,
                    GraphicPaneMeta {
                        pane_id: id,
                        title: title.as_deref(),
                        caption: caption.as_deref(),
                        focused: false,
                        content_hash,
                    },
                    |w, h| {
                        prepare_custom_svg(source, tooltips, selected, &highlighted, &theme, w, h)
                    },
                );
            }
            Node::Split {
                direction: SplitDirection::Horizontal,
                ratio,
                weights,
                children,
            } if !children.is_empty() => {
                let constraints = split_constraints(*ratio, weights, children.len());
                let chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints(constraints)
                    .split(slice.dest_rect);
                for (i, (child, chunk)) in children.iter().zip(chunks.iter()).enumerate() {
                    self.draw_scroll_child(
                        frame,
                        child,
                        ScrollSlice {
                            dest_rect: *chunk,
                            full_height: slice.full_height,
                            clip_top: slice.clip_top,
                        },
                        &format!("{path}.{i}"),
                        state,
                    );
                }
            }
            _ => {
                let full_rect = Rect::new(0, 0, width, slice.full_height);
                let mut offscreen = Buffer::empty(full_rect);
                let bg = to_ratatui_color(self.theme.bg);
                for y in 0..full_rect.height {
                    for x in 0..full_rect.width {
                        if let Some(cell) = offscreen.cell_mut((x, y)) {
                            cell.set_bg(bg);
                        }
                    }
                }

                match node {
                    Node::Markdown {
                        title,
                        tone,
                        content,
                        by_selected,
                        ..
                    } => {
                        let borderless = title.is_none() && tone.is_none();
                        self.render_markdown_into_buffer(
                            &mut offscreen,
                            full_rect,
                            MarkdownRenderArgs {
                                title: title.as_deref(),
                                tone: *tone,
                                content,
                                by_selected,
                                borderless,
                                focused: false,
                                scroll: 0,
                            },
                            state,
                        );
                    }
                    Node::Code {
                        id,
                        title,
                        lang,
                        path: file_path,
                        lines,
                        source,
                        start_line,
                        highlight_lines,
                        focus_key,
                        by_selected,
                        ..
                    } => {
                        let pane_key = id.clone().unwrap_or_else(|| path.to_string());
                        self.render_code_into_buffer(
                            &mut offscreen,
                            full_rect,
                            CodeRenderArgs {
                                pane_key: &pane_key,
                                title: title.as_deref(),
                                lang: lang.as_deref(),
                                path: file_path.as_deref(),
                                lines: *lines,
                                source,
                                start_line: *start_line,
                                highlight_lines,
                                focus_key: focus_key.as_deref(),
                                by_selected,
                                focused: false,
                            },
                            state,
                        );
                    }
                    Node::Stat { title, items, .. } => {
                        render_stat_into_buffer(
                            &mut offscreen,
                            full_rect,
                            title.as_deref(),
                            items,
                            false,
                            &self.theme,
                        );
                    }
                    Node::Table {
                        id,
                        title,
                        columns,
                        rows,
                        ..
                    } => {
                        let pane_key = id.clone().unwrap_or_else(|| path.to_string());
                        let cursor = self
                            .table_cursor
                            .get(&pane_key)
                            .copied()
                            .unwrap_or(0)
                            .min(rows.len().saturating_sub(1));
                        let selected_id = state.get("selected").and_then(|v| v.as_str());
                        let tbl_out = render_table_into_buffer(
                            &mut offscreen,
                            full_rect,
                            &TableRenderArgs {
                                title: title.as_deref(),
                                columns,
                                rows,
                                selected_id,
                                cursor,
                                scroll: 0,
                                focused: false,
                            },
                            &self.theme,
                        );
                        let row_ids: Vec<Option<String>> =
                            rows.iter().map(|r| r.id.clone()).collect();
                        self.table_lengths.insert(pane_key.clone(), rows.len());
                        self.table_row_ids.insert(pane_key.clone(), row_ids.clone());
                        let row_y_spans: Vec<(usize, u16, u16)> = tbl_out
                            .row_y_spans
                            .into_iter()
                            .filter_map(|(idx, y0, y1)| {
                                let vis_y0 = y0.max(slice.clip_top);
                                let vis_y1 = y1.min(slice.clip_top + slice.dest_rect.height);
                                if vis_y1 <= vis_y0 {
                                    return None;
                                }
                                Some((
                                    idx,
                                    slice.dest_rect.y + (vis_y0 - slice.clip_top),
                                    slice.dest_rect.y + (vis_y1 - slice.clip_top),
                                ))
                            })
                            .collect();
                        if !row_y_spans.is_empty() {
                            self.table_areas.push(TableHitArea {
                                table_id: pane_key,
                                inner_rect: slice.dest_rect,
                                row_ids,
                                row_y_spans,
                            });
                        }
                    }
                    Node::Log {
                        id, title, entries, ..
                    } => {
                        let follow = *self.log_follow.get(id).unwrap_or(&true);
                        let count = render_log_into_buffer(
                            &mut offscreen,
                            full_rect,
                            &LogRenderArgs {
                                title: title.as_deref(),
                                entries,
                                min_level: self.log_min_level,
                                search_query: &self.search_query,
                                scroll: 0,
                                follow,
                                focused: false,
                            },
                            &self.theme,
                        );
                        self.log_counts.insert(id.clone(), count);
                    }
                    _ => {}
                }

                blit_buffer_rows(
                    &offscreen,
                    slice.clip_top,
                    slice.dest_rect,
                    frame.buffer_mut(),
                );
            }
        }
    }

    fn measure_node_height(&self, node: &Node, width: u16, state: &StateMap) -> u16 {
        match node {
            Node::Markdown {
                title,
                tone,
                height,
                content,
                by_selected,
                ..
            } => {
                if let Some(h) = height {
                    return (*h).max(1);
                }
                let selected = state.get("selected").and_then(|v| v.as_str());
                let body = selected
                    .and_then(|sel| by_selected.get(sel))
                    .map(String::as_str)
                    .unwrap_or(content.as_str());
                let borderless = title.is_none() && tone.is_none();
                let inner_w = if borderless {
                    width.max(1)
                } else {
                    width.saturating_sub(2).max(1)
                };
                let lines = render_markdown_lines(body, &self.theme);
                let wrapped = wrapped_line_count(&lines, inner_w);
                let chrome = if borderless { 0 } else { 2 };
                (wrapped + chrome).max(1)
            }
            Node::Stat { height, .. } => height.unwrap_or(3).max(1),
            Node::Chart {
                height, caption, ..
            }
            | Node::Graph {
                height, caption, ..
            }
            | Node::Svg {
                height, caption, ..
            } => height.unwrap_or(14).max(4) + u16::from(caption.is_some()),
            Node::Table { rows, height, .. } => {
                height.unwrap_or_else(|| (rows.len() as u16 + 3).min(16))
            }
            Node::Code {
                source,
                by_selected,
                height,
                ..
            } => {
                if let Some(h) = height {
                    return (*h).max(3);
                }
                let selected = state.get("selected").and_then(|v| v.as_str());
                let active_source = selected
                    .and_then(|sel| by_selected.get(sel))
                    .map(|v| v.source.as_str())
                    .unwrap_or(source.as_str());
                (active_source.lines().count().max(1) as u16 + 2).min(20)
            }
            Node::Log { height, .. } => height.unwrap_or(12).max(3),
            Node::Split {
                direction,
                children,
                ..
            } => match direction {
                SplitDirection::Horizontal => children
                    .iter()
                    .map(|c| {
                        self.measure_node_height(
                            c,
                            (width / children.len().max(1) as u16).max(1),
                            state,
                        )
                    })
                    .max()
                    .unwrap_or(4),
                SplitDirection::Vertical => children
                    .iter()
                    .map(|c| self.measure_node_height(c, width, state))
                    .sum::<u16>()
                    .max(4),
            },
            Node::Tabs { id, items } => {
                let active = self
                    .store
                    .active_tabs
                    .get(id)
                    .copied()
                    .unwrap_or(0)
                    .min(items.len().saturating_sub(1));
                items
                    .get(active)
                    .map(|t| self.measure_node_height(&t.node, width, state) + 1)
                    .unwrap_or(4)
            }
            Node::Scroll { children, .. } => children
                .iter()
                .map(|c| self.measure_node_height(c, width, state))
                .sum::<u16>()
                .max(4),
        }
    }

    fn render_markdown_into_buffer(
        &self,
        buf: &mut Buffer,
        area: Rect,
        args: MarkdownRenderArgs<'_>,
        state: &StateMap,
    ) -> (u16, usize) {
        let selected = state.get("selected").and_then(|v| v.as_str());
        let body = selected
            .and_then(|sel| args.by_selected.get(sel))
            .map(String::as_str)
            .unwrap_or(args.content);
        let lines = render_markdown_lines(body, &self.theme);

        if args.borderless {
            let total = usize::from(wrapped_line_count(&lines, area.width));
            let clamped = args
                .scroll
                .min(total.saturating_sub(usize::from(area.height)) as u16);
            Paragraph::new(lines)
                .style(
                    Style::default()
                        .bg(to_ratatui_color(self.theme.bg))
                        .fg(to_ratatui_color(self.theme.fg)),
                )
                .wrap(Wrap { trim: false })
                .scroll((clamped, 0))
                .render(area, buf);
            return (area.height, total);
        }

        let block = match args.tone {
            Some(tone) => callout_block(args.title, tone, &self.theme),
            None => pane_block(args.title, args.focused, &self.theme),
        };
        let inner = block.inner(area);
        let total = usize::from(wrapped_line_count(&lines, inner.width));
        let clamped = args
            .scroll
            .min(total.saturating_sub(usize::from(inner.height)) as u16);
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((clamped, 0))
            .render(area, buf);
        (inner.height, total)
    }

    fn render_code_into_buffer(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        args: CodeRenderArgs<'_>,
        state: &StateMap,
    ) -> (u16, usize) {
        let selected = state.get("selected").and_then(|v| v.as_str());
        let (
            mut active_lang,
            active_path,
            active_lines,
            active_source,
            mut active_start,
            mut active_hl,
        ) = if let Some(variant) = selected.and_then(|sel| args.by_selected.get(sel)) {
            (
                variant.lang.as_deref().or(args.lang),
                variant.path.as_deref(),
                variant.lines,
                variant.source.as_str(),
                variant.start_line,
                variant.highlight_lines.clone(),
            )
        } else {
            (
                args.lang,
                args.path,
                args.lines,
                args.source,
                args.start_line,
                args.highlight_lines.to_vec(),
            )
        };

        let mut loaded_owned: Option<String> = None;
        if let Some(p) = active_path {
            match resolve_code_file(p, active_lines, self.store.base_dir.as_deref()) {
                Ok((content, start)) => {
                    loaded_owned = Some(content);
                    active_start = start;
                    if active_lang.is_none() {
                        active_lang = infer_lang_from_path(p);
                    }
                }
                Err(err) => {
                    let block = pane_block(args.title, args.focused, &self.theme);
                    let inner = block.inner(area);
                    Paragraph::new(format!("Cannot read `{p}`: {err}"))
                        .block(block)
                        .style(Style::default().fg(to_ratatui_color(self.theme.error)))
                        .wrap(Wrap { trim: false })
                        .render(area, buf);
                    return (inner.height, 1);
                }
            }
        }
        let active_source = loaded_owned.as_deref().unwrap_or(active_source);

        let focus_range = args
            .focus_key
            .and_then(|k| state.get(k))
            .and_then(|v| v.as_range());
        if let Some(range) = focus_range {
            active_hl.push(range);
        }

        let prev_focus = self.last_code_focus.get(args.pane_key).copied().flatten();
        if focus_range != prev_focus {
            self.last_code_focus
                .insert(args.pane_key.to_string(), focus_range);
            if let Some((start, _)) = focus_range {
                let target = start.saturating_sub(active_start).saturating_sub(2);
                self.scrolls.insert(args.pane_key.to_string(), target);
            }
        }

        let scroll = self.scrolls.get(args.pane_key).copied().unwrap_or(0) as u16;
        let h_scroll = self.h_scrolls.get(args.pane_key).copied().unwrap_or(0) as u16;
        let decorated_title = if !self.code_wrap {
            let suffix = if h_scroll > 0 {
                format!("[nowrap col+{h_scroll}]")
            } else {
                "[nowrap]".to_string()
            };
            Some(match args.title {
                Some(t) => format!("{t} {suffix}"),
                None => suffix,
            })
        } else {
            args.title.map(ToString::to_string)
        };
        let block = pane_block(decorated_title.as_deref(), args.focused, &self.theme);
        let inner = block.inner(area);
        let (lines, x_scroll) = if self.code_wrap {
            (
                render_code_lines_wrapped(
                    active_source,
                    active_lang,
                    active_start,
                    &active_hl,
                    &self.theme,
                    usize::from(inner.width),
                ),
                0,
            )
        } else {
            (
                render_code_lines(
                    active_source,
                    active_lang,
                    active_start,
                    &active_hl,
                    &self.theme,
                ),
                h_scroll,
            )
        };
        let total = lines.len();
        let max_scroll = total.saturating_sub(usize::from(inner.height));
        let clamped = usize::from(scroll).min(max_scroll);
        if clamped != usize::from(scroll) {
            self.scrolls.insert(args.pane_key.to_string(), clamped);
        }
        Paragraph::new(lines)
            .block(block)
            .scroll((clamped as u16, x_scroll))
            .render(area, buf);
        (inner.height, total)
    }

    fn draw_graphic_pane(
        &mut self,
        frame: &mut Frame<'_>,
        slice: ScrollSlice,
        meta: GraphicPaneMeta<'_>,
        render_bundle: impl FnOnce(u32, u32) -> Result<SvgBundle, String>,
    ) {
        let pane_id = meta.pane_id;
        let content_hash = meta.content_hash;
        let width = slice.dest_rect.width;
        if width < 3 || slice.full_height < 3 || slice.dest_rect.height == 0 {
            return;
        }

        let has_caption = meta.caption.is_some() && slice.full_height >= 4;
        let box_h = if has_caption {
            slice.full_height - 1
        } else {
            slice.full_height
        };
        let inner_w = width.saturating_sub(2);
        let inner_h = box_h.saturating_sub(2);

        let full_rect = Rect::new(0, 0, width, slice.full_height);
        let mut chrome = Buffer::empty(full_rect);
        let bg = to_ratatui_color(self.theme.bg);
        for y in 0..full_rect.height {
            for x in 0..full_rect.width {
                if let Some(cell) = chrome.cell_mut((x, y)) {
                    cell.set_bg(bg);
                }
            }
        }

        let block = pane_block(meta.title, meta.focused, &self.theme);
        block.render(Rect::new(0, 0, width, box_h), &mut chrome);

        if let Some(cap) = meta.caption.filter(|_| has_caption) {
            Paragraph::new(Line::from(Span::styled(
                format!(" {cap}"),
                Style::default()
                    .fg(to_ratatui_color(self.theme.muted))
                    .add_modifier(Modifier::ITALIC),
            )))
            .style(Style::default().bg(bg))
            .render(Rect::new(0, box_h, width, 1), &mut chrome);
        }

        blit_buffer_rows(&chrome, slice.clip_top, slice.dest_rect, frame.buffer_mut());

        if inner_w == 0 || inner_h == 0 {
            return;
        }

        let cell_w = u32::from(self.cell_px.width.max(1));
        let cell_h = u32::from(self.cell_px.height.max(1));
        let full_px_w = u32::from(inner_w) * cell_w;
        let full_px_h = u32::from(inner_h) * cell_h;

        let vis_top = slice.clip_top;
        let vis_bot = slice.clip_top + slice.dest_rect.height;
        let int_start = vis_top.max(1);
        let int_end = vis_bot.min(1 + inner_h);
        if int_end <= int_start {
            return;
        }

        let int_clip_top = int_start - 1;
        let int_vis_h = int_end - int_start;
        let dest_inner_y = slice.dest_rect.y + (int_start - vis_top);
        let vis_inner = Rect::new(slice.dest_rect.x + 1, dest_inner_y, inner_w, int_vis_h);

        // Author SVGs in logical 1x CSS pixels (assuming ~18px/cell at 1x) while rasterizing
        // at full physical resolution so Retina displays get crisp 2x text matching terminal size.
        let dpi_scale = (cell_h as f32 / 18.0).clamp(1.0, 3.0);
        let logical_w = ((full_px_w as f32) / dpi_scale).round().max(120.0) as u32;
        let logical_h = ((full_px_h as f32) / dpi_scale).round().max(90.0) as u32;

        // Always cache at full uncropped dimensions so scrolling crops pixels without re-rasterizing SVGs.
        let key = RasterCacheKey {
            pane_id: pane_id.to_string(),
            theme_mode: self.theme.mode,
            content_hash,
            px_w: full_px_w,
            px_h: full_px_h,
        };

        match self
            .cache
            .get_or_render(key, || render_bundle(logical_w, logical_h))
        {
            Ok(graphic) => {
                let src_y_px = u32::from(int_clip_top) * cell_h;
                let vis_px_h = u32::from(int_vis_h) * cell_h;
                let cropped = int_clip_top > 0 || int_vis_h < inner_h;

                self.graphic_areas.push(GraphicPaneHitArea {
                    pane_id: pane_id.to_string(),
                    inner_rect: vis_inner,
                    hit_map: graphic.hit_map.clone(),
                    px_w: full_px_w,
                    px_h: vis_px_h,
                    px_y_offset: src_y_px,
                });

                if self.kitty_enabled {
                    let buf = frame.buffer_mut();
                    for y in vis_inner.y..vis_inner.bottom() {
                        for x in vis_inner.x..vis_inner.right() {
                            if let Some(cell) = buf.cell_mut((x, y)) {
                                cell.set_char(' ').set_bg(Color::Reset);
                            }
                        }
                    }
                    self.pending_kitty.push(KittyPlacement {
                        pane_id: pane_id.to_string(),
                        content_hash: hash_render_key(&(
                            self.theme.mode,
                            content_hash,
                            src_y_px,
                            vis_px_h,
                        )),
                        rect: vis_inner,
                        graphic: std::sync::Arc::clone(&graphic),
                        crop: cropped.then_some((src_y_px, vis_px_h)),
                    });
                } else if cropped {
                    render_half_blocks_cropped(
                        &graphic.image,
                        src_y_px,
                        vis_px_h,
                        vis_inner,
                        frame.buffer_mut(),
                    );
                } else {
                    render_half_blocks(&graphic.image, vis_inner, frame.buffer_mut());
                }
            }
            Err(err) => {
                let msg = Paragraph::new(err)
                    .style(Style::default().fg(to_ratatui_color(self.theme.error)))
                    .wrap(Wrap { trim: false });
                frame.render_widget(msg, vis_inner);
            }
        }
    }

    fn register_focus(&mut self, key: &str, rect: Rect, kind: PaneKind) -> bool {
        let idx = self.focus_order.len();
        self.focus_order.push(key.to_string());
        self.pane_areas.push(PaneArea {
            key: key.to_string(),
            rect,
            kind,
        });
        idx == self.focused_pane
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.search_mode {
            match key.code {
                KeyCode::Esc => {
                    self.search_mode = false;
                    self.search_query.clear();
                }
                KeyCode::Enter => {
                    self.search_mode = false;
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.search_query.push(c);
                }
                _ => {}
            }
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('p') => {
                self.store.prev_step();
                self.hover = None;
            }
            KeyCode::Char('n') => {
                self.store.next_step();
                self.hover = None;
            }
            KeyCode::Left => {
                if !self.store.doc.steps.is_empty() {
                    self.store.prev_step();
                    self.hover = None;
                } else {
                    self.h_scroll_focused(-4);
                }
            }
            KeyCode::Right => {
                if !self.store.doc.steps.is_empty() {
                    self.store.next_step();
                    self.hover = None;
                } else {
                    self.h_scroll_focused(4);
                }
            }
            KeyCode::Char('h') => self.h_scroll_focused(-4),
            KeyCode::Char('l') => self.h_scroll_focused(4),
            KeyCode::Char('H') => self.h_scroll_focused(-12),
            KeyCode::Char('L') => self.h_scroll_focused(12),
            KeyCode::Tab => {
                if !self.focus_order.is_empty() {
                    self.focused_pane = (self.focused_pane + 1) % self.focus_order.len();
                }
            }
            KeyCode::BackTab => {
                if !self.focus_order.is_empty() {
                    self.focused_pane = if self.focused_pane == 0 {
                        self.focus_order.len() - 1
                    } else {
                        self.focused_pane - 1
                    };
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll_focused(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_focused(-1),
            KeyCode::Char('J') => {
                let half = self.active_scroll_half_page();
                self.scroll_focused(half);
            }
            KeyCode::Char('K') => {
                let half = self.active_scroll_half_page();
                self.scroll_focused(-half);
            }
            KeyCode::PageDown => {
                let page = self.active_scroll_page();
                self.scroll_focused(page);
            }
            KeyCode::PageUp => {
                let page = self.active_scroll_page();
                self.scroll_focused(-page);
            }
            KeyCode::Char('g') => self.jump_focused_top(),
            KeyCode::Char('G') => self.jump_focused_bottom(),
            KeyCode::Enter => self.activate_focused_row(),
            KeyCode::Char('[') => self.cycle_tabs(-1),
            KeyCode::Char(']') => self.cycle_tabs(1),
            KeyCode::Char('t') => {
                self.theme = match self.theme.mode {
                    ThemeMode::Light => Theme::dark(),
                    ThemeMode::Dark | ThemeMode::Auto => Theme::light(),
                };
            }
            KeyCode::Char('w') => {
                self.code_wrap = !self.code_wrap;
            }
            KeyCode::Char('z') => {
                if self.zoomed_pane.is_some() {
                    self.zoomed_pane = None;
                    self.hover = None;
                } else if let Some(pane) = self.pane_areas.get(self.focused_pane)
                    && pane.kind != PaneKind::Scroll
                {
                    self.zoomed_pane = Some(pane.key.clone());
                    self.hover = None;
                }
            }
            KeyCode::Char('/') => {
                self.search_mode = true;
            }
            KeyCode::Char('0') => self.log_min_level = None,
            KeyCode::Char('1') => self.log_min_level = Some(LogLevel::Trace),
            KeyCode::Char('2') => self.log_min_level = Some(LogLevel::Debug),
            KeyCode::Char('3') => self.log_min_level = Some(LogLevel::Info),
            KeyCode::Char('4') => self.log_min_level = Some(LogLevel::Warn),
            KeyCode::Char('5') => self.log_min_level = Some(LogLevel::Error),
            KeyCode::Esc => {
                if self.zoomed_pane.is_some() {
                    self.zoomed_pane = None;
                    self.hover = None;
                } else {
                    self.search_query.clear();
                    self.log_min_level = None;
                    self.store.select(None);
                    self.hover = None;
                }
            }
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::Moved => {
                self.hover = self.hit_test_graphics(col, row).map(|hit| ActiveHover {
                    hit,
                    cursor_col: col,
                    cursor_row: row,
                });
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(tab) = self
                    .tab_areas
                    .iter()
                    .find(|t| point_in_rect(col, row, t.rect))
                    .cloned()
                {
                    self.store.active_tabs.insert(tab.tabs_id, tab.tab_index);
                    self.hover = None;
                    return;
                }

                if let Some((idx, _)) = self
                    .pane_areas
                    .iter()
                    .enumerate()
                    .rfind(|(_, p)| point_in_rect(col, row, p.rect))
                {
                    self.focused_pane = idx;
                }

                if let Some(hit) = self.hit_test_graphics(col, row) {
                    if let Some(id) = hit.id {
                        self.store.select(Some(id));
                    }
                    return;
                }

                if let Some(tbl) = self
                    .table_areas
                    .iter()
                    .find(|t| point_in_rect(col, row, t.inner_rect))
                    .cloned()
                    && let Some(&(row_idx, _, _)) = tbl
                        .row_y_spans
                        .iter()
                        .find(|&&(_, y0, y1)| row >= y0 && row < y1)
                    && row_idx < tbl.row_ids.len()
                {
                    self.table_cursor.insert(tbl.table_id, row_idx);
                    if let Some(id) = tbl.row_ids[row_idx].clone() {
                        self.store.select(Some(id));
                    }
                }
            }
            MouseEventKind::ScrollUp => self.scroll_at(col, row, -3),
            MouseEventKind::ScrollDown => self.scroll_at(col, row, 3),
            _ => {}
        }
    }

    fn hit_test_graphics(&self, col: u16, row: u16) -> Option<HoverHit> {
        let area = self
            .graphic_areas
            .iter()
            .find(|g| point_in_rect(col, row, g.inner_rect))?;
        let rel_col = f32::from(col - area.inner_rect.x) + 0.5;
        let rel_row = f32::from(row - area.inner_rect.y) + 0.5;
        let px = rel_col * (area.px_w as f32 / f32::from(area.inner_rect.width.max(1)));
        let py = rel_row * (area.px_h as f32 / f32::from(area.inner_rect.height.max(1)))
            + area.px_y_offset as f32;
        area.hit_map.hit_at(px, py)
    }

    fn scroll_at(&mut self, col: u16, row: u16, delta: isize) {
        if let Some(vp) = self
            .scroll_viewports
            .iter()
            .find(|v| point_in_rect(col, row, v.rect))
            .cloned()
        {
            self.scroll_viewport(&vp, delta);
            return;
        }
        if let Some((idx, _)) = self
            .pane_areas
            .iter()
            .enumerate()
            .find(|(_, p)| point_in_rect(col, row, p.rect))
        {
            self.focused_pane = idx;
        }
        self.scroll_focused(delta);
    }

    fn active_scroll_half_page(&self) -> isize {
        self.scroll_viewports
            .first()
            .map(|vp| (vp.visible_height / 2).max(1) as isize)
            .unwrap_or(10)
    }

    fn active_scroll_page(&self) -> isize {
        self.scroll_viewports
            .first()
            .map(|vp| vp.visible_height.saturating_sub(2).max(1) as isize)
            .unwrap_or(10)
    }

    fn scroll_viewport(&mut self, vp: &ScrollViewport, delta: isize) {
        let max_scroll = vp.total_height.saturating_sub(vp.visible_height);
        let cur = self.scrolls.get(&vp.id).copied().unwrap_or(vp.scroll) as isize;
        let next = (cur + delta).clamp(0, max_scroll as isize) as usize;
        self.scrolls.insert(vp.id.clone(), next);
        self.hover = None;
    }

    fn scroll_focused(&mut self, delta: isize) {
        let Some(pane) = self.pane_areas.get(self.focused_pane).cloned() else {
            return;
        };
        match pane.kind {
            PaneKind::Scroll => {
                if let Some(vp) = self
                    .scroll_viewports
                    .iter()
                    .find(|v| v.id == pane.key)
                    .cloned()
                {
                    self.scroll_viewport(&vp, delta);
                }
            }
            PaneKind::Table => {
                let len = self.table_lengths.get(&pane.key).copied().unwrap_or(0);
                if len == 0 {
                    return;
                }
                let cur = self.table_cursor.get(&pane.key).copied().unwrap_or(0) as isize;
                let next = (cur + delta).clamp(0, (len - 1) as isize) as usize;
                self.table_cursor.insert(pane.key.clone(), next);
                let visible = self
                    .pane_heights
                    .get(&pane.key)
                    .copied()
                    .unwrap_or(1)
                    .max(1);
                let scroll = self.scrolls.entry(pane.key).or_insert(0);
                if next < *scroll {
                    *scroll = next;
                } else if next >= *scroll + visible {
                    *scroll = next + 1 - visible;
                }
            }
            PaneKind::Log => {
                let count = self.log_counts.get(&pane.key).copied().unwrap_or(0);
                let visible = self
                    .pane_heights
                    .get(&pane.key)
                    .copied()
                    .unwrap_or(1)
                    .max(1);
                let max_scroll = count.saturating_sub(visible);
                let follow = *self.log_follow.get(&pane.key).unwrap_or(&true);
                let cur = if follow {
                    max_scroll
                } else {
                    self.scrolls
                        .get(&pane.key)
                        .copied()
                        .unwrap_or(0)
                        .min(max_scroll)
                };
                let next = (cur as isize + delta).clamp(0, max_scroll as isize) as usize;
                self.scrolls.insert(pane.key.clone(), next);
                self.log_follow.insert(pane.key, next >= max_scroll);
            }
            PaneKind::Text => {
                let total = self.text_lengths.get(&pane.key).copied().unwrap_or(0);
                let visible = self.pane_heights.get(&pane.key).copied().unwrap_or(1);
                let max_scroll = total.saturating_sub(visible);
                let cur = self
                    .scrolls
                    .get(&pane.key)
                    .copied()
                    .unwrap_or(0)
                    .min(max_scroll) as isize;
                let next = (cur + delta).clamp(0, max_scroll as isize) as usize;
                self.scrolls.insert(pane.key, next);
            }
            PaneKind::Graphic | PaneKind::Other => {
                if let Some(vp) = self.scroll_viewports.first().cloned() {
                    self.scroll_viewport(&vp, delta);
                }
            }
        }
    }

    fn h_scroll_focused(&mut self, delta: isize) {
        let Some(pane) = self.pane_areas.get(self.focused_pane) else {
            return;
        };
        if pane.kind != PaneKind::Text {
            return;
        }
        let cur = self.h_scrolls.get(&pane.key).copied().unwrap_or(0) as isize;
        let next = (cur + delta).max(0) as usize;
        self.h_scrolls.insert(pane.key.clone(), next);
    }

    fn jump_focused_top(&mut self) {
        let Some(pane) = self.pane_areas.get(self.focused_pane).cloned() else {
            return;
        };
        match pane.kind {
            PaneKind::Scroll => {
                self.scrolls.insert(pane.key, 0);
                self.hover = None;
            }
            PaneKind::Text => {
                self.scrolls.insert(pane.key, 0);
            }
            PaneKind::Log => {
                self.scrolls.insert(pane.key.clone(), 0);
                self.log_follow.insert(pane.key, false);
            }
            PaneKind::Table => {
                self.table_cursor.insert(pane.key.clone(), 0);
                self.scrolls.insert(pane.key, 0);
            }
            PaneKind::Graphic | PaneKind::Other => {
                if let Some(vp) = self.scroll_viewports.first().cloned() {
                    self.scrolls.insert(vp.id, 0);
                    self.hover = None;
                }
            }
        }
    }

    fn jump_focused_bottom(&mut self) {
        let Some(pane) = self.pane_areas.get(self.focused_pane).cloned() else {
            return;
        };
        match pane.kind {
            PaneKind::Scroll => {
                if let Some(vp) = self
                    .scroll_viewports
                    .iter()
                    .find(|v| v.id == pane.key)
                    .cloned()
                {
                    let max_scroll = vp.total_height.saturating_sub(vp.visible_height);
                    self.scrolls.insert(vp.id, max_scroll);
                    self.hover = None;
                }
            }
            PaneKind::Log => {
                self.log_follow.insert(pane.key, true);
            }
            PaneKind::Text => {
                let total = self.text_lengths.get(&pane.key).copied().unwrap_or(0);
                let visible = self.pane_heights.get(&pane.key).copied().unwrap_or(1);
                self.scrolls.insert(pane.key, total.saturating_sub(visible));
            }
            PaneKind::Table => {
                let len = self.table_lengths.get(&pane.key).copied().unwrap_or(0);
                if len > 0 {
                    self.table_cursor.insert(pane.key, len - 1);
                }
            }
            _ => {
                if let Some(vp) = self.scroll_viewports.first().cloned() {
                    let max_scroll = vp.total_height.saturating_sub(vp.visible_height);
                    self.scrolls.insert(vp.id, max_scroll);
                    self.hover = None;
                }
            }
        }
    }

    fn activate_focused_row(&mut self) {
        let Some(pane) = self.pane_areas.get(self.focused_pane) else {
            return;
        };
        if pane.kind != PaneKind::Table {
            return;
        }
        let cursor = self.table_cursor.get(&pane.key).copied().unwrap_or(0);
        if let Some(id) = self
            .table_row_ids
            .get(&pane.key)
            .and_then(|rows| rows.get(cursor))
            .and_then(|id| id.clone())
        {
            self.store.select(Some(id));
        }
    }

    fn cycle_tabs(&mut self, delta: isize) {
        let Some((tabs_id, count)) = self.tabs_order.first().cloned() else {
            return;
        };
        if count == 0 {
            return;
        }
        let cur = self.store.active_tabs.get(&tabs_id).copied().unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(count as isize) as usize;
        self.store.active_tabs.insert(tabs_id, next);
        self.hover = None;
    }
}

struct MarkdownRenderArgs<'a> {
    title: Option<&'a str>,
    tone: Option<Status>,
    content: &'a str,
    by_selected: &'a BTreeMap<String, String>,
    borderless: bool,
    focused: bool,
    scroll: u16,
}

struct CodeRenderArgs<'a> {
    pane_key: &'a str,
    title: Option<&'a str>,
    lang: Option<&'a str>,
    path: Option<&'a str>,
    lines: Option<(usize, usize)>,
    source: &'a str,
    start_line: usize,
    highlight_lines: &'a [(usize, usize)],
    focus_key: Option<&'a str>,
    by_selected: &'a BTreeMap<String, crate::model::CodeVariant>,
    focused: bool,
}

fn blit_buffer_rows(src: &Buffer, clip_top: u16, dest: Rect, dst: &mut Buffer) {
    for row in 0..dest.height {
        let src_y = clip_top + row;
        if src_y >= src.area.height {
            break;
        }
        for col in 0..dest.width {
            if col >= src.area.width {
                break;
            }
            if let (Some(from), Some(to)) = (
                src.cell((col, src_y)),
                dst.cell_mut((dest.x + col, dest.y + row)),
            ) {
                *to = from.clone();
            }
        }
    }
}

pub struct FollowTarget {
    pub events_path: PathBuf,
    pub byte_offset: u64,
}

struct TerminalGuard {
    kitty: KittyPresenter,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = self.kitty.clear_all(&mut stdout);
        let _ = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

pub fn run_tui(
    store: Store,
    cli_theme: ThemeMode,
    mut follow: Option<FollowTarget>,
) -> miette::Result<()> {
    // Detect terminal background before raw mode / alt screen alters terminal state.
    let env_theme = std::env::var("VIZ_THEME").ok();
    let resolved_mode = resolve_theme_mode(cli_theme, env_theme.as_deref());
    let theme = Theme::for_mode(resolved_mode);

    let cell_px = query_cell_pixel_size();
    let kitty_enabled = io::stdout().is_terminal() && detect_kitty_support();

    enable_raw_mode().into_diagnostic()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).into_diagnostic()?;

    let mut guard = TerminalGuard {
        kitty: KittyPresenter::new(kitty_enabled),
    };

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).into_diagnostic()?;

    let mut app = App::new(store, theme, kitty_enabled, cell_px);

    let (watch_tx, watch_rx) = mpsc::channel::<()>();
    let _watcher = if let Some(ref target) = follow {
        if let Some(parent) = target.events_path.parent() {
            let tx = watch_tx.clone();
            let mut watcher = notify::recommended_watcher(move |res: notify::Result<_>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            })
            .ok();
            if let Some(ref mut w) = watcher {
                let _ = w.watch(parent, RecursiveMode::NonRecursive);
            }
            watcher
        } else {
            None
        }
    } else {
        None
    };

    let poll_interval = if follow.is_some() {
        Duration::from_millis(50)
    } else {
        Duration::from_millis(100)
    };
    let mut dirty = true;

    while !app.should_quit {
        if dirty {
            terminal.draw(|f| app.draw(f)).into_diagnostic()?;
            guard
                .kitty
                .sync(&app.pending_kitty, terminal.backend_mut())
                .into_diagnostic()?;
            dirty = false;
        }

        if event::poll(poll_interval).into_diagnostic()? {
            match event::read().into_diagnostic()? {
                CtEvent::Key(key) => {
                    app.handle_key(key);
                    dirty = true;
                }
                CtEvent::Mouse(mouse) => {
                    app.handle_mouse(mouse);
                    dirty = true;
                }
                CtEvent::Resize(_, _) => {
                    app.cell_px = query_cell_pixel_size();
                    app.hover = None;
                    dirty = true;
                }
                _ => {}
            }
        }

        while watch_rx.try_recv().is_ok() {}
        if let Some(ref mut target) = follow
            && let Ok(events) = read_frames_from(&target.events_path, &mut target.byte_offset)
            && !events.is_empty()
        {
            for ev in events {
                app.store.apply_event(ev);
            }
            app.hover = None;
            dirty = true;
        }
    }

    drop(terminal);
    drop(guard);
    Ok(())
}

fn find_node_by_key<'a>(node: &'a Node, path: &str, target_key: &str) -> Option<&'a Node> {
    match node {
        Node::Split { children, .. } => {
            for (idx, child) in children.iter().enumerate() {
                if let Some(found) = find_node_by_key(child, &format!("{path}.{idx}"), target_key) {
                    return Some(found);
                }
            }
            None
        }
        Node::Scroll { id, children } => {
            if id == target_key {
                return Some(node);
            }
            for (idx, child) in children.iter().enumerate() {
                if let Some(found) = find_node_by_key(child, &format!("{path}.{idx}"), target_key) {
                    return Some(found);
                }
            }
            None
        }
        Node::Tabs { items, .. } => {
            for (idx, item) in items.iter().enumerate() {
                if let Some(found) =
                    find_node_by_key(&item.node, &format!("{path}.tab{idx}"), target_key)
                {
                    return Some(found);
                }
            }
            None
        }
        _ => {
            let key = node.id().unwrap_or(path);
            if key == target_key { Some(node) } else { None }
        }
    }
}

fn wrapped_line_count(lines: &[Line<'_>], width: u16) -> u16 {
    let w = width.max(1);
    let count: u16 = lines
        .iter()
        .map(|l| {
            let lw = l.width();
            if lw == 0 { 1 } else { (lw as u16).div_ceil(w) }
        })
        .sum();
    count.max(1)
}

fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if max_chars == 1 {
        return "…".into();
    }
    let mut out: String = text.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

fn split_constraints(ratio: Option<f32>, weights: &[u16], count: usize) -> Vec<Constraint> {
    if count == 2
        && weights.is_empty()
        && let Some(r) = ratio
    {
        let pct = (r.clamp(0.1, 0.9) * 100.0).round() as u16;
        return vec![
            Constraint::Percentage(pct),
            Constraint::Percentage(100 - pct),
        ];
    }
    if weights.len() == count {
        let total: u32 = weights.iter().map(|&w| u32::from(w.max(1))).sum();
        return weights
            .iter()
            .map(|&w| Constraint::Ratio(u32::from(w.max(1)), total))
            .collect();
    }
    (0..count)
        .map(|_| Constraint::Ratio(1, count.max(1) as u32))
        .collect()
}

fn point_in_rect(col: u16, row: u16, rect: Rect) -> bool {
    col >= rect.x && col < rect.right() && row >= rect.y && row < rect.bottom()
}

fn hash_render_key<T: serde::Serialize>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    if let Ok(bytes) = serde_json::to_vec(value) {
        bytes.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kdl_codec::parse_document;
    use ratatui::backend::TestBackend;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            out.push('\n');
        }
        out
    }

    fn demo_app() -> App {
        let kdl = r##"
            viz v=1 title="System Walkthrough" subtitle="Request path" {
                state selected="gw" highlight="gw" code-focus="1..2"
                steps {
                    step "Ingress" selected="gw" highlight="gw" code-focus="1..2" {
                        markdown "Step one hits **gateway**."
                    }
                    step "Auth Check" selected="auth" highlight="auth" code-focus="3..4" {
                        markdown "Step two validates **JWT**."
                    }
                }
                split direction="horizontal" weights="3,2" {
                    tabs id="main" {
                        tab "Topology" {
                            graph id="g1" title="Arch" direction="lr" {
                                node "gw" label="Gateway" status="ok" tooltip="TLS ingress"
                                node "auth" label="Auth" status="active" tooltip="JWT verifier"
                                edge "gw" "auth" label="gRPC" active=#true
                            }
                        }
                        tab "Latency" {
                            chart id="c1" title="Latency" type="line" x-labels="t0,t1" y-unit="ms" {
                                series "p99" data="12,24"
                            }
                        }
                        tab "Custom" {
                            svg id="s1" title="Diagram" {
                                source #"""
                                <svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100">
                                  <rect id="box1" x="10" y="10" width="180" height="80" fill="var(--accent)"/>
                                </svg>
                                """#
                                tooltip "box1" "Custom box tip"
                            }
                        }
                    }
                    split direction="vertical" weights="1,1,1" {
                        markdown id="md" title="Detail" {
                            content "Default note"
                            by-selected {
                                case "gw" "Gateway details here"
                                case "auth" "Auth details here"
                            }
                        }
                        code id="code" title="Source" lang="rust" focus-key="code-focus" {
                            source #"""
                            fn default_fn() {}
                            """#
                            by-selected {
                                case "gw" lang="rust" {
                                    source #"""
                                    fn handle_gw() {}
                                    """#
                                }
                                case "auth" lang="rust" {
                                    source #"""
                                    fn verify_jwt() {}
                                    """#
                                }
                            }
                        }
                        log id="logs" title="Events" {
                            entry ts="12:00:01" level="debug" source="gw" "debug noise"
                            entry ts="12:00:02" level="info" source="gw" "request accepted"
                            entry ts="12:00:03" level="error" source="auth" "token rejected"
                        }
                    }
                }
            }
        "##;
        let doc = parse_document(kdl, "demo.kdl").unwrap();
        App::new(
            Store::new(doc),
            Theme::dark(),
            false,
            CellPixelSize::default(),
        )
    }

    #[test]
    fn renders_walkthrough_steps_theme_toggle_and_tabs() {
        let mut app = demo_app();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|f| app.draw(f)).unwrap();
        let frame1 = buffer_text(&terminal);
        assert!(frame1.contains("System Walkthrough"));
        assert!(frame1.contains("[1/2] Ingress"));
        assert!(frame1.contains("Gateway details here"));
        assert!(frame1.contains("handle_gw"));
        assert_eq!(app.theme.mode, ThemeMode::Dark);
        assert!(!frame1.contains("[DARK]"));

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        let frame2 = buffer_text(&terminal);
        assert!(frame2.contains("[2/2] Auth Check"));
        assert!(frame2.contains("Auth details here"));
        assert!(frame2.contains("verify_jwt"));

        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.theme.mode, ThemeMode::Light);
        assert!(!buffer_text(&terminal).contains("[LIGHT]"));

        app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.store.active_tabs.get("main"), Some(&1));
        assert!(app.graphic_areas.iter().any(|g| g.pane_id == "c1"));

        app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.store.active_tabs.get("main"), Some(&2));
        assert!(app.graphic_areas.iter().any(|g| g.pane_id == "s1"));
    }

    #[test]
    fn hover_tooltip_and_click_to_select_update_linked_panes() {
        let mut app = demo_app();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|f| app.draw(f)).unwrap();

        let g_area = app
            .graphic_areas
            .iter()
            .find(|g| g.pane_id == "g1")
            .unwrap()
            .clone();
        let auth_target = g_area
            .hit_map
            .targets
            .iter()
            .find(|t| t.id == "auth")
            .unwrap();
        let center_px = auth_target.x + auth_target.w * 0.5;
        let center_py = auth_target.y + auth_target.h * 0.5;
        let col = g_area.inner_rect.x
            + ((center_px / g_area.px_w as f32) * f32::from(g_area.inner_rect.width)) as u16;
        let row = g_area.inner_rect.y
            + ((center_py / g_area.px_h as f32) * f32::from(g_area.inner_rect.height)) as u16;

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.hover.is_some());

        terminal.draw(|f| app.draw(f)).unwrap();
        let with_tip = buffer_text(&terminal);
        assert!(with_tip.contains("JWT verifier"));

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        });
        terminal.draw(|f| app.draw(f)).unwrap();
        let after_click = buffer_text(&terminal);
        assert!(after_click.contains("Auth details here"));
        assert!(after_click.contains("verify_jwt"));

        app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        let c_area = app
            .graphic_areas
            .iter()
            .find(|g| g.pane_id == "c1")
            .unwrap()
            .clone();
        let chart_col = c_area.inner_rect.x + c_area.inner_rect.width / 2;
        let chart_row = c_area.inner_rect.y + c_area.inner_rect.height / 2;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: chart_col,
            row: chart_row,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.hover.is_some());
        terminal.draw(|f| app.draw(f)).unwrap();
        let chart_tip = buffer_text(&terminal);
        assert!(chart_tip.contains("p99:"));

        app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        let s_area = app
            .graphic_areas
            .iter()
            .find(|g| g.pane_id == "s1")
            .unwrap()
            .clone();
        let svg_col = s_area.inner_rect.x + s_area.inner_rect.width / 2;
        let svg_row = s_area.inner_rect.y + s_area.inner_rect.height / 2;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: svg_col,
            row: svg_row,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.hover.is_some());
        terminal.draw(|f| app.draw(f)).unwrap();
        let svg_tip = buffer_text(&terminal);
        assert!(svg_tip.contains("Custom box tip"));
    }

    #[test]
    fn log_level_filter_and_search_filter_entries() {
        let mut app = demo_app();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|f| app.draw(f)).unwrap();
        let all = buffer_text(&terminal);
        assert!(all.contains("debug noise"));
        assert!(all.contains("token rejected"));

        app.handle_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        let errors_only = buffer_text(&terminal);
        assert!(!errors_only.contains("debug noise"));
        assert!(!errors_only.contains("request accepted"));
        assert!(errors_only.contains("token rejected"));

        app.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for ch in "accepted".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        let searched = buffer_text(&terminal);
        assert!(searched.contains("request accepted"));
        assert!(!searched.contains("token rejected"));
    }

    #[test]
    fn scroll_report_clips_partially_and_preserves_hover_accuracy() {
        let kdl = r##"
            viz v=1 title="Scroll Report" {
                scroll id="page" {
                    markdown """
                    # Top Section
                    Intro prose line.
                    """
                    markdown id="finding" title="Finding" tone="warn" """
                    Watch out for cold starts.
                    """
                    stat id="kpis" {
                        item "a" label="Cold" value="42s" status="warn"
                        item "b" label="Warm" value="12s" status="ok"
                    }
                    svg id="diag" title="Two Boxes" caption="Bottom caption text" height=14 {
                        source #"""
                        <svg xmlns="http://www.w3.org/2000/svg" width="200" height="200" viewBox="0 0 200 200">
                          <rect id="top-box" x="10" y="10" width="180" height="80" fill="var(--accent)"/>
                          <rect id="bot-box" x="10" y="110" width="180" height="80" fill="var(--ok)"/>
                        </svg>
                        """#
                        tooltip "top-box" "Upper half box"
                        tooltip "bot-box" "Lower half box"
                    }
                    chart id="bench" title="Timings" type="hbar" x-labels="Alpha,Beta" x-unit="seconds" value-labels=#true caption="Median of 5 runs" height=14 {
                        series "v1" data="10,20"
                        series "v2" data="6,11"
                    }
                    markdown """
                    ## Closing Notes
                    Final paragraph at bottom.
                    """
                }
            }
        "##;
        let doc = parse_document(kdl, "scroll.kdl").unwrap();
        let mut app = App::new(
            Store::new(doc),
            Theme::dark(),
            true,
            CellPixelSize {
                width: 10,
                height: 20,
            },
        );
        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|f| app.draw(f)).unwrap();
        let top_frame = buffer_text(&terminal);
        assert!(top_frame.contains("Top Section"));
        assert!(top_frame.contains("Finding"));
        assert!(top_frame.contains("1/"));
        assert!(!top_frame.contains("Final paragraph at bottom."));

        for _ in 0..15 {
            app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        terminal.draw(|f| app.draw(f)).unwrap();
        let cache_len_after_partial = app.cache.len();

        let diag_area = app
            .graphic_areas
            .iter()
            .find(|g| g.pane_id == "diag")
            .expect("diag graphic area visible")
            .clone();
        assert!(diag_area.px_y_offset > 0);

        let kitty_diag = app
            .pending_kitty
            .iter()
            .find(|p| p.pane_id == "diag")
            .expect("kitty placement for cropped diag");
        let (crop_y, crop_h) = kitty_diag.crop.expect("expected vertical crop");
        assert_eq!(crop_y, diag_area.px_y_offset);
        assert_eq!(crop_h, diag_area.px_h);
        let hash_at_15 = kitty_diag.content_hash;

        let hover_col = diag_area.inner_rect.x + diag_area.inner_rect.width / 2;
        let hover_row = diag_area.inner_rect.y + diag_area.inner_rect.height - 2;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: hover_col,
            row: hover_row,
            modifiers: KeyModifiers::NONE,
        });
        let hit = app.hover.as_ref().expect("hover hit on cropped svg");
        assert_eq!(hit.hit.id.as_deref(), Some("bot-box"));
        assert_eq!(hit.hit.title, "Lower half box");

        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        let kitty_diag_16 = app
            .pending_kitty
            .iter()
            .find(|p| p.pane_id == "diag")
            .unwrap();
        assert_ne!(kitty_diag_16.content_hash, hash_at_15);
        assert_eq!(app.cache.len(), cache_len_after_partial);

        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        terminal.draw(|f| app.draw(f)).unwrap();
        let bottom_frame = buffer_text(&terminal);
        assert!(bottom_frame.contains("Final paragraph at bottom."));

        app.handle_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));
        terminal.draw(|f| app.draw(f)).unwrap();
        assert!(app.scrolls["page"] < app.scroll_viewports[0].total_height);

        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.scrolls["page"], 0);
    }

    #[test]
    fn narrow_header_keeps_badges_unclipped_step_wraps_and_markdown_strips_raw_syntax() {
        let kdl = r##"
            viz v=1 title="Very Long Architecture Walkthrough Title" subtitle="With an even longer subtitle that must truncate" {
                steps {
                    step "Long Step" {
                        markdown "This is a deliberately long walkthrough step explanation that wraps onto a second line on a narrow viewport and ends with UNIQUE_TAIL_MARKER."
                    }
                }
                markdown id="md" title="Notes" """
                ### Clean Heading
                Calls `verify_jwt`, then returns.
                """
            }
        "##;
        let doc = parse_document(kdl, "narrow.kdl").unwrap();
        let mut app = App::new(
            Store::new(doc),
            Theme::light(),
            false,
            CellPixelSize::default(),
        );
        app.store.closed = true;

        let backend = TestBackend::new(48, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();

        let text = buffer_text(&terminal);
        let header_line = text.lines().next().unwrap();
        assert!(header_line.contains("[CLOSED]"));
        assert!(!header_line.contains("[LIVE]"));
        assert!(!header_line.contains("[LIGHT]"));
        assert!(header_line.contains('…'));
        assert!(text.contains("UNIQUE_TAIL_MARKER"));
        assert!(text.contains("Clean Heading"));
        assert!(!text.contains("### "));
        assert!(text.contains("Calls verify_jwt, then returns."));
        assert!(!text.contains("`verify_jwt`"));
    }

    #[test]
    fn responsive_split_stacks_narrow_panes_and_supports_zoom_wrap_and_click_focus() {
        let kdl = r##"
            viz v=1 title="Responsive" {
                split direction="horizontal" weights="1,1" {
                    markdown id="left" title="Left" "Left pane content"
                    code id="right" title="Right" lang="rust" {
                        source #"""
                        pub fn extremely_long_identifier_for_wrapping_check(first: usize) -> usize { first + 1 }
                        """#
                    }
                }
            }
        "##;
        let doc = parse_document(kdl, "resp.kdl").unwrap();
        let mut app = App::new(
            Store::new(doc),
            Theme::dark(),
            false,
            CellPixelSize::default(),
        );

        let mut wide = Terminal::new(TestBackend::new(100, 24)).unwrap();
        wide.draw(|f| app.draw(f)).unwrap();
        let left_wide = app
            .pane_areas
            .iter()
            .find(|p| p.key == "left")
            .unwrap()
            .rect;
        let right_wide = app
            .pane_areas
            .iter()
            .find(|p| p.key == "right")
            .unwrap()
            .rect;
        assert_eq!(left_wide.y, right_wide.y);
        assert!(right_wide.x > left_wide.x);

        let mut narrow = Terminal::new(TestBackend::new(60, 24)).unwrap();
        narrow.draw(|f| app.draw(f)).unwrap();
        let left_narrow = app
            .pane_areas
            .iter()
            .find(|p| p.key == "left")
            .unwrap()
            .rect;
        let right_narrow = app
            .pane_areas
            .iter()
            .find(|p| p.key == "right")
            .unwrap()
            .rect;
        assert_eq!(left_narrow.x, right_narrow.x);
        assert!(right_narrow.y > left_narrow.y);

        let wrapped_text = buffer_text(&narrow);
        assert!(wrapped_text.contains("first + 1"));
        assert!(!wrapped_text.contains("[nowrap]"));

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: right_narrow.x + 2,
            row: right_narrow.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.pane_areas[app.focused_pane].key, "right");

        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        narrow.draw(|f| app.draw(f)).unwrap();
        let unwrapped_text = buffer_text(&narrow);
        assert!(unwrapped_text.contains("[nowrap]"));

        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
        narrow.draw(|f| app.draw(f)).unwrap();
        let zoomed_text = buffer_text(&narrow);
        assert!(zoomed_text.contains("[ZOOM]"));
        assert!(!zoomed_text.contains("Left pane content"));
        assert_eq!(app.pane_areas.len(), 1);
        assert_eq!(app.pane_areas[0].key, "right");

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        narrow.draw(|f| app.draw(f)).unwrap();
        let unzoomed_text = buffer_text(&narrow);
        assert!(!unzoomed_text.contains("[ZOOM]"));
        assert!(unzoomed_text.contains("Left pane content"));
        assert_eq!(app.pane_areas.len(), 2);
    }

    #[test]
    fn text_pane_scroll_clamps_to_last_visible_line() {
        let kdl = r#"
            viz v=1 title="Short Notes" {
                markdown id="notes" title="Component Notes" "Line one\n\nLine two\n\nLine three"
            }
        "#;
        let doc = parse_document(kdl, "notes.kdl").unwrap();
        let mut app = App::new(
            Store::new(doc),
            Theme::light(),
            false,
            CellPixelSize::default(),
        );
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();

        for _ in 0..30 {
            app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        terminal.draw(|f| app.draw(f)).unwrap();

        assert_eq!(app.scrolls.get("notes").copied().unwrap_or(0), 0);
        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Line one"));
        assert!(rendered.contains("Line three"));
    }
}
