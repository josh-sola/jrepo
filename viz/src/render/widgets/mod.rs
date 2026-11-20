pub mod code;
pub mod markdown;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Widget, Wrap};

use crate::model::{LogEntry, LogLevel, StatItem, Status, TableRow};
use crate::svg::HoverHit;
use crate::theme::{Theme, ThemeColor};

const SPARK_BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn to_ratatui_color(c: ThemeColor) -> Color {
    Color::Rgb(c.r(), c.g(), c.b())
}

pub fn pane_block(title: Option<&str>, focused: bool, theme: &Theme) -> Block<'static> {
    let border_color = if focused {
        to_ratatui_color(theme.accent)
    } else {
        to_ratatui_color(theme.border)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(to_ratatui_color(theme.surface)));

    if let Some(t) = title {
        let title_style = if focused {
            Style::default()
                .fg(to_ratatui_color(theme.accent))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(to_ratatui_color(theme.fg))
                .add_modifier(Modifier::BOLD)
        };
        block = block.title(Span::styled(format!(" {t} "), title_style));
    }
    block
}

pub fn callout_block(title: Option<&str>, tone: Status, theme: &Theme) -> Block<'static> {
    let tone_color = to_ratatui_color(theme.status_color(tone));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(tone_color))
        .style(Style::default().bg(to_ratatui_color(theme.surface_alt)));

    if let Some(t) = title {
        block = block.title(Span::styled(
            format!(" {t} "),
            Style::default().fg(tone_color).add_modifier(Modifier::BOLD),
        ));
    }
    block
}

pub fn sparkline_str(values: &[f64]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = max - min;
    values
        .iter()
        .map(|&v| {
            let idx = if span.abs() < 1e-9 {
                3
            } else {
                (((v - min) / span) * 7.0).round().clamp(0.0, 7.0) as usize
            };
            SPARK_BARS[idx]
        })
        .collect()
}

pub fn render_stat_into_buffer(
    buf: &mut Buffer,
    area: Rect,
    title: Option<&str>,
    items: &[StatItem],
    focused: bool,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // In compact 3-row scroll strips with no outer title, draw individual bordered cards directly
    // so label/status sit in the top border and value/delta/sparkline stay visible on the middle row.
    if title.is_none() && area.height <= 3 && !items.is_empty() {
        let constraints: Vec<Constraint> = items
            .iter()
            .map(|_| Constraint::Ratio(1, items.len() as u32))
            .collect();
        let slots = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(area);
        for (item, slot) in items.iter().zip(slots.iter()) {
            let status_color = item
                .status
                .map(|s| to_ratatui_color(theme.status_color(s)))
                .unwrap_or_else(|| to_ratatui_color(theme.accent));
            let border_color = if focused {
                to_ratatui_color(theme.accent)
            } else {
                to_ratatui_color(theme.border)
            };
            let title_label = match item.status {
                Some(st) => format!(" {} [{}] ", item.label, st.as_str()),
                None => format!(" {} ", item.label),
            };
            let mut value_spans = vec![Span::styled(
                item.value.clone(),
                Style::default()
                    .fg(to_ratatui_color(theme.fg))
                    .add_modifier(Modifier::BOLD),
            )];
            if let Some(delta) = &item.delta {
                value_spans.push(Span::raw(" "));
                value_spans.push(Span::styled(
                    delta.clone(),
                    Style::default().fg(status_color),
                ));
            }
            if !item.sparkline.is_empty() {
                value_spans.push(Span::raw(" "));
                value_spans.push(Span::styled(
                    sparkline_str(&item.sparkline),
                    Style::default().fg(status_color),
                ));
            }
            let card = Paragraph::new(Line::from(value_spans)).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color))
                    .style(Style::default().bg(to_ratatui_color(theme.surface_alt)))
                    .title(Span::styled(
                        title_label,
                        Style::default().fg(to_ratatui_color(theme.muted)),
                    )),
            );
            card.render(*slot, buf);
        }
        return;
    }

    let block = pane_block(title, focused, theme);
    let inner = block.inner(area);
    block.render(area, buf);

    if items.is_empty() || inner.width == 0 || inner.height == 0 {
        return;
    }

    let horizontal = inner.width >= (items.len() as u16).saturating_mul(16);
    let constraints: Vec<Constraint> = items
        .iter()
        .map(|_| Constraint::Ratio(1, items.len() as u32))
        .collect();
    let slots = Layout::default()
        .direction(if horizontal {
            Direction::Horizontal
        } else {
            Direction::Vertical
        })
        .constraints(constraints)
        .split(inner);

    for (item, slot) in items.iter().zip(slots.iter()) {
        let status_color = item
            .status
            .map(|s| to_ratatui_color(theme.status_color(s)))
            .unwrap_or_else(|| to_ratatui_color(theme.accent));

        if slot.height <= 2 {
            let mut spans = vec![
                Span::styled(
                    format!("{}: ", item.label),
                    Style::default().fg(to_ratatui_color(theme.muted)),
                ),
                Span::styled(
                    item.value.clone(),
                    Style::default()
                        .fg(to_ratatui_color(theme.fg))
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            if let Some(delta) = &item.delta {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    delta.clone(),
                    Style::default().fg(status_color),
                ));
            }
            if !item.sparkline.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    sparkline_str(&item.sparkline),
                    Style::default().fg(status_color),
                ));
            }
            Paragraph::new(Line::from(spans))
                .style(Style::default().bg(to_ratatui_color(theme.surface_alt)))
                .render(*slot, buf);
            continue;
        }

        let mut header_spans = vec![Span::styled(
            item.label.clone(),
            Style::default().fg(to_ratatui_color(theme.muted)),
        )];
        if let Some(st) = item.status {
            header_spans.push(Span::raw(" "));
            header_spans.push(Span::styled(
                format!("[{}]", st.as_str()),
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        let mut value_spans = vec![Span::styled(
            item.value.clone(),
            Style::default()
                .fg(to_ratatui_color(theme.fg))
                .add_modifier(Modifier::BOLD),
        )];
        if let Some(delta) = &item.delta {
            value_spans.push(Span::raw(" "));
            value_spans.push(Span::styled(
                delta.clone(),
                Style::default().fg(status_color),
            ));
        }

        let mut lines = vec![Line::from(header_spans), Line::from(value_spans)];
        if !item.sparkline.is_empty() {
            lines.push(Line::from(Span::styled(
                sparkline_str(&item.sparkline),
                Style::default().fg(status_color),
            )));
        }

        let card = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(to_ratatui_color(theme.border)))
                    .style(Style::default().bg(to_ratatui_color(theme.surface_alt))),
            )
            .wrap(Wrap { trim: false });
        card.render(*slot, buf);
    }
}

pub struct TableRenderArgs<'a> {
    pub title: Option<&'a str>,
    pub columns: &'a [String],
    pub rows: &'a [TableRow],
    pub selected_id: Option<&'a str>,
    pub cursor: usize,
    pub scroll: usize,
    pub focused: bool,
}

pub struct TableRenderOutput {
    pub inner: Rect,
    pub row_y_spans: Vec<(usize, u16, u16)>,
}

pub fn compute_waterfill_widths(
    columns: &[String],
    rows: &[TableRow],
    avail_w: u16,
) -> Option<Vec<u16>> {
    let col_count = columns
        .len()
        .max(rows.iter().map(|r| r.cells.len()).max().unwrap_or(0));
    if col_count == 0 {
        return Some(Vec::new());
    }

    let gaps = (col_count as u16).saturating_sub(1);
    let usable = avail_w.saturating_sub(gaps);
    if usable < col_count as u16 * 3 {
        return None;
    }

    let mut natural = vec![3u16; col_count];
    for (c, header) in columns.iter().enumerate() {
        natural[c] = natural[c].max(header.chars().count() as u16);
    }
    for row in rows {
        for (c, cell) in row.cells.iter().enumerate() {
            if c < col_count {
                natural[c] = natural[c].max(cell.chars().count() as u16);
            }
        }
    }

    let min_w: Vec<u16> = natural.iter().map(|&n| n.clamp(3, 10)).collect();
    let min_sum: u16 = min_w.iter().sum();
    if col_count >= 3 && min_sum > usable {
        return None;
    }

    let nat_sum: u16 = natural.iter().sum();
    if nat_sum <= usable {
        let mut widths = natural;
        let mut slack = usable - nat_sum;
        let mut order: Vec<usize> = (0..col_count).collect();
        order.sort_by(|&a, &b| widths[b].cmp(&widths[a]));
        while slack > 0 {
            for &idx in &order {
                if slack == 0 {
                    break;
                }
                widths[idx] += 1;
                slack -= 1;
            }
        }
        return Some(widths);
    }

    let mut widths = min_w;
    let mut rem = usable.saturating_sub(min_sum);
    while rem > 0 {
        let mut progressed = false;
        for c in 0..col_count {
            if rem == 0 {
                break;
            }
            if widths[c] < natural[c] {
                widths[c] += 1;
                rem -= 1;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    Some(widths)
}

pub fn render_table_into_buffer(
    buf: &mut Buffer,
    area: Rect,
    args: &TableRenderArgs<'_>,
    theme: &Theme,
) -> TableRenderOutput {
    let block = pane_block(args.title, args.focused, theme);
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.width == 0 || inner.height == 0 {
        return TableRenderOutput {
            inner,
            row_y_spans: Vec::new(),
        };
    }

    let Some(col_widths) = compute_waterfill_widths(args.columns, args.rows, inner.width) else {
        return render_stacked_table_cards(buf, inner, args, theme);
    };

    let widths: Vec<Constraint> = col_widths.into_iter().map(Constraint::Length).collect();

    let header = Row::new(args.columns.iter().map(|c| {
        Cell::from(c.clone()).style(
            Style::default()
                .fg(to_ratatui_color(theme.accent))
                .add_modifier(Modifier::BOLD),
        )
    }))
    .style(Style::default().bg(to_ratatui_color(theme.surface_alt)));

    let mut row_y_spans = Vec::new();
    let max_rows = usize::from(inner.height.saturating_sub(1));
    let visible_rows = args
        .rows
        .iter()
        .enumerate()
        .skip(args.scroll)
        .take(max_rows)
        .enumerate()
        .map(|(vis_i, (idx, row))| {
            let y = inner.y + 1 + vis_i as u16;
            row_y_spans.push((idx, y, y + 1));

            let is_selected = args.selected_id.is_some() && row.id.as_deref() == args.selected_id;
            let is_cursor = args.focused && idx == args.cursor;

            let fg = row
                .status
                .map(|s| to_ratatui_color(theme.status_color(s)))
                .unwrap_or_else(|| to_ratatui_color(theme.fg));
            let mut style = Style::default().fg(fg);
            if is_selected || is_cursor {
                style = style
                    .bg(to_ratatui_color(theme.selection))
                    .add_modifier(Modifier::BOLD);
            }

            Row::new(row.cells.iter().map(|c| Cell::from(c.clone()))).style(style)
        });

    let table = Table::new(visible_rows, widths)
        .header(header)
        .column_spacing(1);
    Widget::render(table, inner, buf);
    TableRenderOutput { inner, row_y_spans }
}

fn render_stacked_table_cards(
    buf: &mut Buffer,
    inner: Rect,
    args: &TableRenderArgs<'_>,
    theme: &Theme,
) -> TableRenderOutput {
    let mut row_y_spans = Vec::new();
    let mut y = inner.y;
    let bottom = inner.bottom();

    for (idx, row) in args.rows.iter().enumerate().skip(args.scroll) {
        if y >= bottom {
            break;
        }
        let card_start = y;
        let is_selected = args.selected_id.is_some() && row.id.as_deref() == args.selected_id;
        let is_cursor = args.focused && idx == args.cursor;
        let primary_fg = row
            .status
            .map(|s| to_ratatui_color(theme.status_color(s)))
            .unwrap_or_else(|| to_ratatui_color(theme.fg));
        let bg = if is_selected || is_cursor {
            to_ratatui_color(theme.selection)
        } else {
            to_ratatui_color(theme.surface)
        };

        let mut card_lines: Vec<Line<'static>> = Vec::new();
        let primary = row.cells.first().cloned().unwrap_or_default();
        card_lines.push(Line::from(Span::styled(
            primary,
            Style::default()
                .fg(primary_fg)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )));

        for (c_idx, val) in row.cells.iter().enumerate().skip(1) {
            let col_label = args.columns.get(c_idx).map(String::as_str).unwrap_or("");
            card_lines.push(Line::from(vec![
                Span::styled(
                    format!("  {col_label}: "),
                    Style::default().fg(to_ratatui_color(theme.muted)).bg(bg),
                ),
                Span::styled(
                    val.clone(),
                    Style::default().fg(to_ratatui_color(theme.fg)).bg(bg),
                ),
            ]));
        }

        let card_h = (card_lines.len() as u16).min(bottom - y);
        let card_rect = Rect::new(inner.x, y, inner.width, card_h);
        Paragraph::new(card_lines)
            .style(Style::default().bg(bg))
            .wrap(Wrap { trim: false })
            .render(card_rect, buf);
        y += card_h;
        row_y_spans.push((idx, card_start, y));

        if y < bottom {
            let sep_rect = Rect::new(inner.x, y, inner.width, 1);
            let rule = "─".repeat(usize::from(inner.width));
            Paragraph::new(Line::from(Span::styled(
                rule,
                Style::default().fg(to_ratatui_color(theme.border)),
            )))
            .render(sep_rect, buf);
            y += 1;
        }
    }

    TableRenderOutput { inner, row_y_spans }
}

pub struct LogRenderArgs<'a> {
    pub title: Option<&'a str>,
    pub entries: &'a [LogEntry],
    pub min_level: Option<LogLevel>,
    pub search_query: &'a str,
    pub scroll: usize,
    pub follow: bool,
    pub focused: bool,
}

pub fn filter_log_entries<'a>(
    entries: &'a [LogEntry],
    min_level: Option<LogLevel>,
    search_query: &str,
) -> Vec<&'a LogEntry> {
    let min_rank = min_level.map(level_rank).unwrap_or(0);
    let needle = search_query.trim().to_ascii_lowercase();
    entries
        .iter()
        .filter(|e| level_rank(e.level) >= min_rank)
        .filter(|e| {
            if needle.is_empty() {
                return true;
            }
            e.message.to_ascii_lowercase().contains(&needle)
                || e.source
                    .as_deref()
                    .is_some_and(|s| s.to_ascii_lowercase().contains(&needle))
        })
        .collect()
}

pub fn render_log_into_buffer(
    buf: &mut Buffer,
    area: Rect,
    args: &LogRenderArgs<'_>,
    theme: &Theme,
) -> usize {
    let filtered = filter_log_entries(args.entries, args.min_level, args.search_query);
    let mut title_parts = Vec::new();
    if let Some(t) = args.title {
        title_parts.push(t.to_string());
    } else {
        title_parts.push("Logs".into());
    }
    if let Some(lvl) = args.min_level {
        title_parts.push(format!(">={}", lvl.as_str()));
    }
    if !args.search_query.is_empty() {
        title_parts.push(format!("/{}", args.search_query));
    }
    if !args.follow {
        title_parts.push("PAUSED".into());
    }
    let header_title = title_parts.join(" · ");

    let block = pane_block(Some(&header_title), args.focused, theme);
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.width == 0 || inner.height == 0 {
        return filtered.len();
    }

    let visible_h = usize::from(inner.height);
    let max_scroll = filtered.len().saturating_sub(visible_h);
    let effective_scroll = if args.follow {
        max_scroll
    } else {
        args.scroll.min(max_scroll)
    };

    let lines: Vec<Line<'static>> = filtered
        .iter()
        .skip(effective_scroll)
        .take(visible_h)
        .map(|entry| {
            let mut spans = Vec::new();
            if let Some(ts) = &entry.ts {
                spans.push(Span::styled(
                    format!("{ts} "),
                    Style::default().fg(to_ratatui_color(theme.muted)),
                ));
            }
            let lvl_color = match entry.level {
                LogLevel::Trace | LogLevel::Debug => to_ratatui_color(theme.muted),
                LogLevel::Info => to_ratatui_color(theme.active),
                LogLevel::Warn => to_ratatui_color(theme.warn),
                LogLevel::Error => to_ratatui_color(theme.error),
            };
            spans.push(Span::styled(
                format!("{:<5} ", entry.level.as_str().to_ascii_uppercase()),
                Style::default().fg(lvl_color).add_modifier(Modifier::BOLD),
            ));
            if let Some(src) = &entry.source {
                spans.push(Span::styled(
                    format!("[{src}] "),
                    Style::default().fg(to_ratatui_color(theme.accent)),
                ));
            }
            spans.push(Span::styled(
                entry.message.clone(),
                Style::default().fg(to_ratatui_color(theme.fg)),
            ));
            Line::from(spans)
        })
        .collect();

    Paragraph::new(lines).render(inner, buf);
    filtered.len()
}

pub fn render_tooltip_overlay(
    frame: &mut Frame<'_>,
    hit: &HoverHit,
    cursor_col: u16,
    cursor_row: u16,
    theme: &Theme,
) {
    let screen = frame.area();
    if screen.width < 10 || screen.height < 4 {
        return;
    }

    let mut content_lines = vec![Line::from(Span::styled(
        format!(" {} ", hit.title),
        Style::default()
            .fg(to_ratatui_color(theme.accent))
            .add_modifier(Modifier::BOLD),
    ))];
    for line in &hit.lines {
        content_lines.push(Line::from(Span::styled(
            format!(" {line} "),
            Style::default().fg(to_ratatui_color(theme.fg)),
        )));
    }

    let max_text_w = content_lines
        .iter()
        .map(|l| l.width())
        .max()
        .unwrap_or(10)
        .min(52);
    let box_w = (max_text_w as u16 + 2).min(screen.width);
    let inner_w = box_w.saturating_sub(2).max(1) as usize;
    let wrapped_rows: u16 = content_lines
        .iter()
        .map(|l| l.width().max(1).div_ceil(inner_w) as u16)
        .sum();
    let box_h = (wrapped_rows + 2).min(screen.height);

    let x = if cursor_col.saturating_add(2).saturating_add(box_w) <= screen.width {
        cursor_col.saturating_add(2)
    } else {
        cursor_col
            .saturating_sub(box_w)
            .min(screen.width.saturating_sub(box_w))
    };
    let y = if cursor_row.saturating_add(1).saturating_add(box_h) <= screen.height {
        cursor_row.saturating_add(1)
    } else {
        cursor_row
            .saturating_sub(box_h)
            .min(screen.height.saturating_sub(box_h))
    };

    let popup = Rect::new(x, y, box_w, box_h);
    let surface_bg = to_ratatui_color(theme.surface);
    frame.render_widget(Clear, popup);
    let card = Paragraph::new(content_lines)
        .style(Style::default().bg(surface_bg))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(
                    Style::default()
                        .fg(to_ratatui_color(theme.accent))
                        .bg(surface_bg),
                )
                .style(Style::default().bg(surface_bg)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(card, popup);
}

pub const fn level_rank(level: LogLevel) -> u8 {
    match level {
        LogLevel::Trace => 1,
        LogLevel::Debug => 2,
        LogLevel::Info => 3,
        LogLevel::Warn => 4,
        LogLevel::Error => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    #[test]
    fn waterfill_gives_short_columns_natural_width_and_falls_back_to_stacked_cards() {
        let cols = vec!["Pod".into(), "Version".into(), "State".into(), "CPU".into()];
        let rows = vec![
            TableRow {
                id: Some("r1".into()),
                status: Some(Status::Ok),
                cells: vec![
                    "api-canary-7c9d".into(),
                    "v2.14.0".into(),
                    "healthy".into(),
                    "42%".into(),
                ],
            },
            TableRow {
                id: Some("r2".into()),
                status: Some(Status::Warn),
                cells: vec![
                    "api-stable-9b2e".into(),
                    "v2.13.4".into(),
                    "restarting".into(),
                    "81%".into(),
                ],
            },
        ];

        let widths = compute_waterfill_widths(&cols, &rows, 44).expect("fits tabular");
        assert!(widths[0] > widths[3]);
        assert!(widths[3] >= 3);

        assert!(compute_waterfill_widths(&cols, &rows, 22).is_none());

        let area = Rect::new(0, 0, 24, 12);
        let mut buf = Buffer::empty(area);
        let out = render_table_into_buffer(
            &mut buf,
            area,
            &TableRenderArgs {
                title: Some("Pods"),
                columns: &cols,
                rows: &rows,
                selected_id: Some("r2"),
                cursor: 0,
                scroll: 0,
                focused: true,
            },
            &Theme::dark(),
        );
        assert_eq!(out.row_y_spans.len(), 2);
        assert!(out.row_y_spans[0].2 > out.row_y_spans[0].1 + 1);
        let mut text = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                text.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }
        assert!(text.contains("api-canary-7c9d"));
        assert!(text.contains("CPU: 42%"));
    }
}
