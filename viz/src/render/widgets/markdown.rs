use pulldown_cmark::{Event as MdEvent, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::render::widgets::to_ratatui_color;
use crate::theme::Theme;

pub fn render_markdown_lines(source: &str, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default().fg(to_ratatui_color(theme.fg))];
    let mut in_code_block = false;
    let mut quote_depth = 0usize;
    let mut list_stack: Vec<Option<u64>> = Vec::new();

    let flush = |lines: &mut Vec<Line<'static>>, current: &mut Vec<Span<'static>>| {
        if !current.is_empty() {
            lines.push(Line::from(std::mem::take(current)));
        }
    };

    for event in Parser::new(source) {
        match event {
            MdEvent::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush(&mut lines, &mut current);
                    let color = match level {
                        HeadingLevel::H1 => to_ratatui_color(theme.accent),
                        HeadingLevel::H2 => to_ratatui_color(theme.fg),
                        _ => to_ratatui_color(theme.muted),
                    };
                    let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                    style_stack.push(style);
                }
                Tag::Strong => {
                    let top = *style_stack.last().unwrap_or(&Style::default());
                    style_stack.push(top.add_modifier(Modifier::BOLD));
                }
                Tag::Emphasis => {
                    let top = *style_stack.last().unwrap_or(&Style::default());
                    style_stack.push(top.add_modifier(Modifier::ITALIC));
                }
                Tag::BlockQuote(_) => {
                    flush(&mut lines, &mut current);
                    quote_depth += 1;
                    style_stack.push(
                        Style::default()
                            .fg(to_ratatui_color(theme.muted))
                            .add_modifier(Modifier::ITALIC),
                    );
                }
                Tag::CodeBlock(_) => {
                    flush(&mut lines, &mut current);
                    in_code_block = true;
                }
                Tag::List(start) => {
                    flush(&mut lines, &mut current);
                    list_stack.push(start);
                }
                Tag::Item => {
                    flush(&mut lines, &mut current);
                    let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                    let bullet = match list_stack.last_mut() {
                        Some(Some(num)) => {
                            let label = format!("{indent}{num}. ");
                            *num += 1;
                            label
                        }
                        _ => format!("{indent}• "),
                    };
                    current.push(Span::styled(
                        bullet,
                        Style::default().fg(to_ratatui_color(theme.accent)),
                    ));
                }
                Tag::Link { .. } => {
                    style_stack.push(
                        Style::default()
                            .fg(to_ratatui_color(theme.accent))
                            .add_modifier(Modifier::UNDERLINED),
                    );
                }
                Tag::Paragraph if quote_depth > 0 && current.is_empty() => {
                    current.push(Span::styled(
                        "│ ".repeat(quote_depth),
                        Style::default().fg(to_ratatui_color(theme.border)),
                    ));
                }
                _ => {}
            },
            MdEvent::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    style_stack.pop();
                    flush(&mut lines, &mut current);
                    lines.push(Line::default());
                }
                TagEnd::Paragraph => {
                    flush(&mut lines, &mut current);
                    lines.push(Line::default());
                }
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Link => {
                    style_stack.pop();
                }
                TagEnd::BlockQuote(_) => {
                    quote_depth = quote_depth.saturating_sub(1);
                    style_stack.pop();
                    flush(&mut lines, &mut current);
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    flush(&mut lines, &mut current);
                    lines.push(Line::default());
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    flush(&mut lines, &mut current);
                    if list_stack.is_empty() {
                        lines.push(Line::default());
                    }
                }
                TagEnd::Item => {
                    flush(&mut lines, &mut current);
                }
                _ => {}
            },
            MdEvent::Text(text) => {
                if in_code_block {
                    let code_style = Style::default()
                        .fg(to_ratatui_color(theme.fg))
                        .bg(to_ratatui_color(theme.surface_alt));
                    for line in text.lines() {
                        lines.push(Line::from(Span::styled(format!("  {line}  "), code_style)));
                    }
                } else {
                    let style = *style_stack.last().unwrap_or(&Style::default());
                    current.push(Span::styled(text.into_string(), style));
                }
            }
            MdEvent::Code(code) => {
                let style = Style::default()
                    .fg(to_ratatui_color(theme.accent))
                    .bg(to_ratatui_color(theme.surface_alt));
                current.push(Span::styled(code.into_string(), style));
            }
            MdEvent::SoftBreak => {
                current.push(Span::raw(" "));
            }
            MdEvent::HardBreak => {
                flush(&mut lines, &mut current);
            }
            MdEvent::Rule => {
                flush(&mut lines, &mut current);
                lines.push(Line::from(Span::styled(
                    "────────────────────────",
                    Style::default().fg(to_ratatui_color(theme.border)),
                )));
            }
            _ => {}
        }
    }

    flush(&mut lines, &mut current);
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
    lines
}
