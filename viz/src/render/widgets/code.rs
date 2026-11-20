use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::render::widgets::to_ratatui_color;
use crate::theme::Theme;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(ThemeSet::load_defaults)
}

pub fn render_code_lines(
    source: &str,
    lang: Option<&str>,
    start_line: usize,
    highlight_ranges: &[(usize, usize)],
    theme: &Theme,
) -> Vec<Line<'static>> {
    render_code_lines_impl(source, lang, start_line, highlight_ranges, theme, None)
}

pub fn render_code_lines_wrapped(
    source: &str,
    lang: Option<&str>,
    start_line: usize,
    highlight_ranges: &[(usize, usize)],
    theme: &Theme,
    max_width: usize,
) -> Vec<Line<'static>> {
    render_code_lines_impl(
        source,
        lang,
        start_line,
        highlight_ranges,
        theme,
        Some(max_width),
    )
}

fn render_code_lines_impl(
    source: &str,
    lang: Option<&str>,
    start_line: usize,
    highlight_ranges: &[(usize, usize)],
    theme: &Theme,
    wrap_width: Option<usize>,
) -> Vec<Line<'static>> {
    let ss = syntax_set();
    let ts = theme_set();
    let syntax = lang
        .and_then(|token| {
            ss.find_syntax_by_token(token)
                .or_else(|| ss.find_syntax_by_name(token))
        })
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let syntect_theme = ts
        .themes
        .get(theme.syntect_theme_name())
        .or_else(|| ts.themes.values().next());

    let total_lines = source.lines().count().max(1);
    let max_line_no = start_line.saturating_add(total_lines);
    let gutter_w = max_line_no.to_string().len().max(2);
    let gutter_cols = gutter_w + 2;
    let content_limit = wrap_width.map(|w| w.saturating_sub(gutter_cols).max(8));

    let mut highlighter = syntect_theme.map(|t| HighlightLines::new(syntax, t));
    let mut rendered = Vec::with_capacity(total_lines);

    for (idx, raw_line) in LinesWithEndings::from(source).enumerate() {
        let line_no = start_line + idx;
        let highlighted = highlight_ranges
            .iter()
            .any(|&(lo, hi)| line_no >= lo && line_no <= hi);

        let gutter_style = if highlighted {
            Style::default()
                .fg(to_ratatui_color(theme.highlight))
                .bg(to_ratatui_color(theme.selection))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(to_ratatui_color(theme.muted))
        };

        let marker = if highlighted { "▌" } else { " " };
        let first_gutter = Span::styled(format!("{marker}{line_no:>gutter_w$}│"), gutter_style);
        let cont_gutter = Span::styled(format!("{marker}{}│", " ".repeat(gutter_w)), gutter_style);

        let clean = raw_line.trim_end_matches(['\r', '\n']);
        let mut styled_tokens: Vec<(String, Style)> = Vec::new();
        if let Some(hl) = highlighter.as_mut()
            && let Ok(ranges) = hl.highlight_line(raw_line, ss)
        {
            for (style, fragment) in ranges {
                let text = fragment.trim_end_matches(['\r', '\n']);
                if text.is_empty() {
                    continue;
                }
                let mut cell_style = Style::default().fg(Color::Rgb(
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                ));
                if highlighted {
                    cell_style = cell_style.bg(to_ratatui_color(theme.selection));
                }
                styled_tokens.push((text.to_string(), cell_style));
            }
        } else {
            let mut fallback = Style::default().fg(to_ratatui_color(theme.fg));
            if highlighted {
                fallback = fallback.bg(to_ratatui_color(theme.selection));
            }
            if !clean.is_empty() {
                styled_tokens.push((clean.to_string(), fallback));
            }
        }

        let Some(limit) = content_limit else {
            let mut spans = Vec::with_capacity(styled_tokens.len() + 1);
            spans.push(first_gutter);
            for (text, style) in styled_tokens {
                spans.push(Span::styled(text, style));
            }
            rendered.push(Line::from(spans));
            continue;
        };

        let mut row_spans = vec![first_gutter];
        let mut row_cols = 0usize;
        for (text, style) in styled_tokens {
            let mut chunk = String::new();
            for ch in text.chars() {
                let ch_w = if ch == '\t' { 4 } else { 1 };
                if row_cols + ch_w > limit && row_cols > 0 {
                    if !chunk.is_empty() {
                        row_spans.push(Span::styled(std::mem::take(&mut chunk), style));
                    }
                    rendered.push(Line::from(std::mem::take(&mut row_spans)));
                    row_spans.push(cont_gutter.clone());
                    row_cols = 0;
                }
                if ch == '\t' {
                    chunk.push_str("    ");
                } else {
                    chunk.push(ch);
                }
                row_cols += ch_w;
            }
            if !chunk.is_empty() {
                row_spans.push(Span::styled(chunk, style));
            }
        }
        rendered.push(Line::from(row_spans));
    }

    if rendered.is_empty() {
        rendered.push(Line::from(Span::styled(
            "(empty)",
            Style::default().fg(to_ratatui_color(theme.muted)),
        )));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_code_preserves_content_and_aligns_continuation_gutter() {
        let src =
            "pub fn very_long_function_name(alpha: usize, beta: usize) -> usize { alpha + beta }";
        let lines =
            render_code_lines_wrapped(src, Some("rust"), 10, &[(10, 10)], &Theme::dark(), 30);
        assert!(lines.len() >= 3);

        let first_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let cont_text: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first_text.starts_with("▌10│"));
        assert!(cont_text.starts_with("▌  │"));

        let reconstructed: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .skip(1)
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(reconstructed, src);
    }
}
