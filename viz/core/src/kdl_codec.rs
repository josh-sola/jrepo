use std::collections::BTreeMap;
use std::fmt::Write as _;

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use miette::{Diagnostic, NamedSource, SourceSpan};

use crate::model::{
    ChartPoint, ChartSeries, ChartType, CodeVariant, Document, EdgeStyle, Event, GraphDirection,
    GraphEdge, GraphNode, LogEntry, LogLevel, Node, NodeShape, SplitDirection, StatItem, StateMap,
    StateValue, Status, Step, TabItem, TableRow,
};

#[derive(Debug, Diagnostic)]
struct CodecError {
    message: String,
    #[source_code]
    src: NamedSource<String>,
    #[label("{label}")]
    span: SourceSpan,
    label: String,
    #[help]
    help: Option<String>,
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CodecError {}

struct ParserCtx<'a> {
    name: &'a str,
    src: &'a str,
}

impl<'a> ParserCtx<'a> {
    fn err(
        &self,
        span: SourceSpan,
        message: impl Into<String>,
        label: impl Into<String>,
        help: Option<String>,
    ) -> miette::Report {
        CodecError {
            message: message.into(),
            src: NamedSource::new(self.name, self.src.to_string()),
            span,
            label: label.into(),
            help,
        }
        .into()
    }
}

pub fn parse_document(src: &str, name: &str) -> miette::Result<Document> {
    if src.trim_start().starts_with('{') {
        return parse_json_document(src, name);
    }

    let doc = parse_kdl(src, name)?;
    let ctx = ParserCtx { name, src };
    let nodes = doc.nodes();
    if nodes.len() != 1 || nodes[0].name().value() != "viz" {
        let span = nodes
            .first()
            .map(|n| n.span())
            .unwrap_or_else(|| (0, 0).into());
        return Err(ctx.err(
            span,
            "expected a single top-level `viz` node",
            "document root must be `viz`",
            Some("Wrap the document in `viz v=1 title=\"...\" { ... }`.".into()),
        ));
    }
    parse_viz_node(&ctx, &nodes[0])
}

pub fn parse_events(src: &str, name: &str) -> miette::Result<Vec<Event>> {
    let trimmed = src.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return parse_json_events(src, name);
    }

    let doc = parse_kdl(src, name)?;
    let ctx = ParserCtx { name, src };
    let mut events = Vec::with_capacity(doc.nodes().len());
    for node in doc.nodes() {
        events.push(parse_event_node(&ctx, node)?);
    }
    Ok(events)
}

fn parse_kdl(src: &str, name: &str) -> miette::Result<KdlDocument> {
    KdlDocument::parse(src).map_err(|err| {
        let span = err
            .diagnostics
            .first()
            .map(|d| d.span)
            .unwrap_or_else(|| (0, 0).into());
        let label = err
            .diagnostics
            .first()
            .and_then(|d| d.label.clone())
            .unwrap_or_else(|| "KDL syntax error".into());
        let help = err.diagnostics.first().and_then(|d| d.help.clone());
        CodecError {
            message: format!("failed to parse KDL: {err}"),
            src: NamedSource::new(name, src.to_string()),
            span,
            label,
            help,
        }
        .into()
    })
}

fn parse_json_document(src: &str, name: &str) -> miette::Result<Document> {
    let doc: Document = serde_json::from_str(src).map_err(|e| {
        let offset = line_col_to_offset(src, e.line(), e.column());
        CodecError {
            message: format!("failed to parse JSON document: {e}"),
            src: NamedSource::new(name, src.to_string()),
            span: (offset, 1).into(),
            label: "invalid JSON document".into(),
            help: None,
        }
    })?;
    if doc.v != 1 {
        return Err(CodecError {
            message: format!("unsupported document version `{}`", doc.v),
            src: NamedSource::new(name, src.to_string()),
            span: (0, 1).into(),
            label: "expected `v: 1`".into(),
            help: Some("Only schema version 1 is supported.".into()),
        }
        .into());
    }
    Ok(doc)
}

fn parse_json_events(src: &str, name: &str) -> miette::Result<Vec<Event>> {
    let trimmed = src.trim_start();
    if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<Event>>(src).map_err(|e| json_event_err(src, name, e))
    } else if let Ok(ev) = serde_json::from_str::<Event>(src) {
        Ok(vec![ev])
    } else {
        let doc = parse_json_document(src, name)?;
        Ok(vec![Event::Init(Box::new(doc))])
    }
}

fn json_event_err(src: &str, name: &str, e: serde_json::Error) -> miette::Report {
    let offset = line_col_to_offset(src, e.line(), e.column());
    CodecError {
        message: format!("failed to parse JSON event: {e}"),
        src: NamedSource::new(name, src.to_string()),
        span: (offset, 1).into(),
        label: "invalid JSON event".into(),
        help: None,
    }
    .into()
}

fn line_col_to_offset(src: &str, line: usize, col: usize) -> usize {
    let mut offset = 0usize;
    for (current_line, chunk) in (1usize..).zip(src.split_inclusive('\n')) {
        if current_line == line {
            return (offset + col.saturating_sub(1)).min(src.len());
        }
        offset += chunk.len();
    }
    src.len().saturating_sub(1)
}

fn parse_viz_node(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Document> {
    let v = match node.get("v") {
        Some(val) => {
            let n = val_as_u32(ctx, prop_span(node, "v"), val, "v")?;
            if n != 1 {
                return Err(ctx.err(
                    prop_span(node, "v"),
                    format!("unsupported document version `{n}`"),
                    "only v=1 is supported",
                    Some("Set `v=1` on the root `viz` node.".into()),
                ));
            }
            n
        }
        None => 1,
    };

    let title = opt_str_prop_or_arg(ctx, node, "title", 0)?.ok_or_else(|| {
        ctx.err(
            node.span(),
            "missing required `title` on `viz`",
            "add `title=\"...\"`",
            None,
        )
    })?;
    let subtitle = opt_str_prop(ctx, node, "subtitle")?;
    let width = opt_fraction_prop(ctx, node, "width")?;
    let height = opt_fraction_prop(ctx, node, "height")?;

    let mut state = StateMap::new();
    let mut steps = Vec::new();
    let mut root: Option<Node> = None;

    let children = node.children().map(|d| d.nodes()).unwrap_or(&[]);
    for child in children {
        match child.name().value() {
            "state" => {
                parse_state_entries(ctx, child, &mut state, &[])?;
            }
            "steps" => {
                for step_node in child.children().map(|d| d.nodes()).unwrap_or(&[]) {
                    if step_node.name().value() != "step" {
                        return Err(ctx.err(
                            step_node.span(),
                            format!(
                                "unexpected node `{}` inside `steps`",
                                step_node.name().value()
                            ),
                            "expected `step`",
                            None,
                        ));
                    }
                    steps.push(parse_step(ctx, step_node)?);
                }
            }
            "step" => {
                steps.push(parse_step(ctx, child)?);
            }
            _ => {
                if root.is_some() {
                    return Err(ctx.err(
                        child.span(),
                        "multiple root visual nodes in `viz`",
                        "only one root node is allowed",
                        Some("Wrap sibling panes inside a `split` or `tabs` node.".into()),
                    ));
                }
                root = Some(parse_node(ctx, child)?);
            }
        }
    }

    let root = root.ok_or_else(|| {
        ctx.err(
            node.span(),
            "`viz` document has no root node",
            "expected a layout or visual primitive child",
            None,
        )
    })?;

    Ok(Document {
        v,
        title,
        subtitle,
        width,
        height,
        state,
        steps,
        root,
    })
}

fn opt_fraction_prop(
    ctx: &ParserCtx<'_>,
    node: &KdlNode,
    key: &str,
) -> miette::Result<Option<f64>> {
    let Some(val) = opt_f64_prop(ctx, node, key)? else {
        return Ok(None);
    };
    if !(0.15..=0.98).contains(&val) {
        return Err(ctx.err(
            prop_span(node, key),
            format!("`{key}` value `{val}` is out of range"),
            "expected a fraction in 0.15..=0.98",
            None,
        ));
    }
    Ok(Some(val))
}

fn parse_step(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Step> {
    let title = opt_str_prop_or_arg(ctx, node, "title", 0)?.ok_or_else(|| {
        ctx.err(
            node.span(),
            "missing required title on `step`",
            "expected `step \"Title\"`",
            None,
        )
    })?;
    let id = opt_str_prop(ctx, node, "id")?;
    let mut body = opt_str_arg(ctx, node, 1)?.unwrap_or_default();
    let mut state = StateMap::new();
    parse_state_entries(ctx, node, &mut state, &["id", "title"])?;

    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        match child.name().value() {
            "markdown" | "body" => {
                body = req_str_arg_or_prop(ctx, child, 0, "content")?;
            }
            "state" => {
                parse_state_entries(ctx, child, &mut state, &[])?;
            }
            other => {
                return Err(ctx.err(
                    child.span(),
                    format!("unknown child `{other}` in `step`"),
                    "expected `markdown`, `body`, or `state`",
                    None,
                ));
            }
        }
    }

    Ok(Step {
        id,
        title,
        body,
        state,
    })
}

fn parse_state_entries(
    ctx: &ParserCtx<'_>,
    node: &KdlNode,
    out: &mut StateMap,
    skip_keys: &[&str],
) -> miette::Result<()> {
    for entry in node.entries() {
        let Some(name) = entry.name() else {
            continue;
        };
        let key = name.value();
        if skip_keys.contains(&key) {
            continue;
        }
        let value = parse_state_value(ctx, entry, key)?;
        out.insert(key.to_string(), value);
    }
    Ok(())
}

fn parse_state_value(
    ctx: &ParserCtx<'_>,
    entry: &KdlEntry,
    key: &str,
) -> miette::Result<StateValue> {
    match entry.value() {
        KdlValue::Bool(b) => Ok(StateValue::Bool(*b)),
        KdlValue::Integer(i) => {
            let v = i64::try_from(*i).map_err(|_| {
                ctx.err(
                    entry.span(),
                    format!("integer out of i64 range for `{key}`"),
                    "integer overflow",
                    None,
                )
            })?;
            Ok(StateValue::Int(v))
        }
        KdlValue::Float(f) => Ok(StateValue::Float(*f)),
        KdlValue::String(s) => Ok(parse_string_state_value(key, s)),
        KdlValue::Null => Ok(StateValue::Str(String::new())),
    }
}

fn parse_string_state_value(key: &str, s: &str) -> StateValue {
    if let Some((start, end)) = parse_line_range(s) {
        return StateValue::Range { start, end };
    }
    if key == "highlight" || s.contains(',') {
        let items: Vec<String> = s
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(ToString::to_string)
            .collect();
        return StateValue::List(items);
    }
    StateValue::Str(s.to_string())
}

fn parse_line_range(s: &str) -> Option<(usize, usize)> {
    let (lhs, rhs) = s.trim().split_once("..")?;
    let start = lhs.trim().parse::<usize>().ok()?;
    let end = rhs.trim().parse::<usize>().ok()?;
    Some((start, end))
}

fn parse_node(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    match node.name().value() {
        "split" => parse_split(ctx, node),
        "scroll" => parse_scroll(ctx, node),
        "tabs" => parse_tabs(ctx, node),
        "markdown" => parse_markdown(ctx, node),
        "code" => parse_code(ctx, node),
        "stat" => parse_stat(ctx, node),
        "table" => parse_table(ctx, node),
        "log" => parse_log(ctx, node),
        "graph" => parse_graph(ctx, node),
        "chart" => parse_chart(ctx, node),
        "svg" => parse_svg(ctx, node),
        other => Err(ctx.err(
            node.span(),
            format!("unknown node kind `{other}`"),
            "unsupported primitive",
            Some(
                "Valid nodes: split, scroll, tabs, markdown, code, stat, table, log, graph, chart, svg."
                    .into(),
            ),
        )),
    }
}

fn parse_scroll(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = opt_str_prop(ctx, node, "id")?.unwrap_or_else(|| "scroll".into());
    let mut children = Vec::new();
    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        children.push(parse_node(ctx, child)?);
    }
    Ok(Node::Scroll { id, children })
}

fn parse_split(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let dir_str = opt_str_prop_or_arg(ctx, node, "direction", 0)?;
    let direction = match dir_str {
        Some(raw) => SplitDirection::parse(&raw).ok_or_else(|| {
            ctx.err(
                prop_or_arg_span(node, "direction", 0),
                format!("invalid split direction `{raw}`"),
                "expected `horizontal` or `vertical`",
                None,
            )
        })?,
        None => SplitDirection::Horizontal,
    };

    let ratio = opt_f64_prop(ctx, node, "ratio")?.map(|v| v as f32);
    let weights = match opt_str_prop(ctx, node, "weights")? {
        Some(raw) => parse_u16_csv(ctx, prop_span(node, "weights"), &raw)?,
        None => Vec::new(),
    };

    let mut children = Vec::new();
    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        children.push(parse_node(ctx, child)?);
    }
    if children.is_empty() {
        return Err(ctx.err(
            node.span(),
            "`split` must contain at least one child node",
            "empty split container",
            None,
        ));
    }

    Ok(Node::Split {
        direction,
        ratio,
        weights,
        children,
    })
}

fn parse_tabs(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = req_id(ctx, node, "tabs")?;
    let mut items = Vec::new();
    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        if child.name().value() != "tab" {
            return Err(ctx.err(
                child.span(),
                format!("unexpected child `{}` in `tabs`", child.name().value()),
                "expected `tab`",
                None,
            ));
        }
        let tab_title = opt_str_prop_or_arg(ctx, child, "title", 0)?.ok_or_else(|| {
            ctx.err(
                child.span(),
                "missing required title on `tab`",
                "expected `tab \"Title\" { ... }`",
                None,
            )
        })?;
        let tab_id = opt_str_prop(ctx, child, "id")?;
        let tab_children = child.children().map(|d| d.nodes()).unwrap_or(&[]);
        if tab_children.len() != 1 {
            return Err(ctx.err(
                child.span(),
                "`tab` must contain exactly one child node",
                "expected one child node",
                None,
            ));
        }
        items.push(TabItem {
            id: tab_id,
            title: tab_title,
            node: parse_node(ctx, &tab_children[0])?,
        });
    }
    Ok(Node::Tabs { id, items })
}

fn parse_markdown(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = opt_str_prop(ctx, node, "id")?;
    let title = opt_str_prop(ctx, node, "title")?;
    let tone = opt_status_prop(ctx, node, "tone")?;
    let height = opt_u16_prop(ctx, node, "height")?;
    let mut content = opt_str_arg(ctx, node, 0)?.unwrap_or_default();
    let mut by_selected = BTreeMap::new();

    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        match child.name().value() {
            "content" => {
                content = req_str_arg_or_prop(ctx, child, 0, "text")?;
            }
            "by-selected" => {
                for case in child.children().map(|d| d.nodes()).unwrap_or(&[]) {
                    if case.name().value() != "case" {
                        return Err(ctx.err(
                            case.span(),
                            format!(
                                "unexpected child `{}` in `by-selected`",
                                case.name().value()
                            ),
                            "expected `case`",
                            None,
                        ));
                    }
                    let key = opt_str_prop_or_arg(ctx, case, "id", 0)?.ok_or_else(|| {
                        ctx.err(
                            case.span(),
                            "missing key on `case`",
                            "expected `case \"id\" \"...\"`",
                            None,
                        )
                    })?;
                    let body = if let Some(s) = opt_str_arg(ctx, case, 1)? {
                        s
                    } else if let Some(sub) = find_child(case, "content") {
                        req_str_arg_or_prop(ctx, sub, 0, "text")?
                    } else {
                        return Err(ctx.err(
                            case.span(),
                            "missing markdown content on `case`",
                            "provide body arg or `content` child",
                            None,
                        ));
                    };
                    by_selected.insert(key, body);
                }
            }
            other => {
                return Err(ctx.err(
                    child.span(),
                    format!("unknown child `{other}` in `markdown`"),
                    "expected `content` or `by-selected`",
                    None,
                ));
            }
        }
    }

    Ok(Node::Markdown {
        id,
        title,
        tone,
        height,
        content,
        by_selected,
    })
}

fn parse_code(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = opt_str_prop(ctx, node, "id")?;
    let title = opt_str_prop(ctx, node, "title")?;
    let lang = opt_str_prop(ctx, node, "lang")?;
    let path = opt_str_prop(ctx, node, "path")?;
    let lines = parse_opt_lines_prop(ctx, node, "lines")?;
    let height = opt_u16_prop(ctx, node, "height")?;
    let start_line = opt_usize_prop(ctx, node, "start-line")?.unwrap_or(1);
    let highlight_lines = match opt_str_prop(ctx, node, "highlight-lines")? {
        Some(raw) => parse_highlight_ranges(ctx, prop_span(node, "highlight-lines"), &raw)?,
        None => Vec::new(),
    };
    let focus_key = opt_str_prop(ctx, node, "focus-key")?;
    let mut source = opt_str_arg(ctx, node, 0)?.unwrap_or_default();
    let mut by_selected = BTreeMap::new();

    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        match child.name().value() {
            "source" => {
                source = req_str_arg_or_prop(ctx, child, 0, "text")?;
            }
            "by-selected" => {
                for case in child.children().map(|d| d.nodes()).unwrap_or(&[]) {
                    if case.name().value() != "case" {
                        return Err(ctx.err(
                            case.span(),
                            format!(
                                "unexpected child `{}` in `by-selected`",
                                case.name().value()
                            ),
                            "expected `case`",
                            None,
                        ));
                    }
                    let key = opt_str_prop_or_arg(ctx, case, "id", 0)?.ok_or_else(|| {
                        ctx.err(
                            case.span(),
                            "missing key on `case`",
                            "expected `case \"id\" ...`",
                            None,
                        )
                    })?;
                    let v_lang = opt_str_prop(ctx, case, "lang")?;
                    let v_path = opt_str_prop(ctx, case, "path")?;
                    let v_lines = parse_opt_lines_prop(ctx, case, "lines")?;
                    let v_start = opt_usize_prop(ctx, case, "start-line")?.unwrap_or(1);
                    let v_hl = match opt_str_prop(ctx, case, "highlight-lines")? {
                        Some(raw) => {
                            parse_highlight_ranges(ctx, prop_span(case, "highlight-lines"), &raw)?
                        }
                        None => Vec::new(),
                    };
                    let v_source = if let Some(s) = opt_str_arg(ctx, case, 1)? {
                        s
                    } else if let Some(sub) = find_child(case, "source") {
                        req_str_arg_or_prop(ctx, sub, 0, "text")?
                    } else if v_path.is_some() {
                        String::new()
                    } else {
                        return Err(ctx.err(
                            case.span(),
                            "missing `source` or `path` on code `case`",
                            "provide `path=\"...\"`, source arg, or `source` child",
                            None,
                        ));
                    };
                    by_selected.insert(
                        key,
                        CodeVariant {
                            lang: v_lang,
                            path: v_path,
                            lines: v_lines,
                            source: v_source,
                            start_line: v_start,
                            highlight_lines: v_hl,
                        },
                    );
                }
            }
            other => {
                return Err(ctx.err(
                    child.span(),
                    format!("unknown child `{other}` in `code`"),
                    "expected `source` or `by-selected`",
                    None,
                ));
            }
        }
    }

    Ok(Node::Code {
        id,
        title,
        lang,
        path,
        lines,
        height,
        source,
        start_line,
        highlight_lines,
        focus_key,
        by_selected,
    })
}

fn parse_opt_lines_prop(
    ctx: &ParserCtx<'_>,
    node: &KdlNode,
    key: &str,
) -> miette::Result<Option<(usize, usize)>> {
    let Some(raw) = opt_str_prop(ctx, node, key)? else {
        return Ok(None);
    };
    if let Some(range) = parse_line_range(&raw) {
        return Ok(Some(range));
    }
    if let Ok(line) = raw.trim().parse::<usize>() {
        return Ok(Some((line, line)));
    }
    Err(ctx.err(
        prop_span(node, key),
        format!("invalid line range `{raw}`"),
        "expected `start..end` or line number",
        None,
    ))
}

fn parse_stat(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = opt_str_prop(ctx, node, "id")?;
    let title = opt_str_prop(ctx, node, "title")?;
    let height = opt_u16_prop(ctx, node, "height")?;
    let mut items = Vec::new();

    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        if child.name().value() != "item" {
            return Err(ctx.err(
                child.span(),
                format!("unknown child `{}` in `stat`", child.name().value()),
                "expected `item`",
                None,
            ));
        }
        let item_id = opt_str_prop_or_arg(ctx, child, "id", 0)?.ok_or_else(|| {
            ctx.err(
                child.span(),
                "missing required `id` on stat `item`",
                "add positional id or `id=\"...\"`",
                None,
            )
        })?;
        let label = opt_str_prop(ctx, child, "label")?.unwrap_or_else(|| item_id.clone());
        let value = opt_str_prop_or_arg(ctx, child, "value", 1)?.ok_or_else(|| {
            ctx.err(
                child.span(),
                "missing required `value` on stat `item`",
                "add `value=\"...\"`",
                None,
            )
        })?;
        let delta = opt_str_prop(ctx, child, "delta")?;
        let status = opt_status_prop(ctx, child, "status")?;
        let sparkline = match opt_str_prop(ctx, child, "sparkline")? {
            Some(raw) => parse_f64_csv(ctx, prop_span(child, "sparkline"), &raw)?,
            None => Vec::new(),
        };
        items.push(StatItem {
            id: item_id,
            label,
            value,
            delta,
            status,
            sparkline,
        });
    }

    Ok(Node::Stat {
        id,
        title,
        height,
        items,
    })
}

fn parse_table(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = opt_str_prop(ctx, node, "id")?;
    let title = opt_str_prop(ctx, node, "title")?;
    let height = opt_u16_prop(ctx, node, "height")?;
    let mut columns = match opt_str_prop(ctx, node, "columns")? {
        Some(raw) => split_csv(&raw),
        None => Vec::new(),
    };
    let mut rows = Vec::new();

    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        match child.name().value() {
            "columns" => {
                columns = positional_strings(ctx, child)?;
            }
            "row" => {
                let row_id = opt_str_prop(ctx, child, "id")?;
                let status = opt_status_prop(ctx, child, "status")?;
                let cells = positional_strings(ctx, child)?;
                rows.push(TableRow {
                    id: row_id,
                    status,
                    cells,
                });
            }
            other => {
                return Err(ctx.err(
                    child.span(),
                    format!("unknown child `{other}` in `table`"),
                    "expected `columns` or `row`",
                    None,
                ));
            }
        }
    }

    Ok(Node::Table {
        id,
        title,
        height,
        columns,
        rows,
    })
}

fn parse_log(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = req_id(ctx, node, "log")?;
    let title = opt_str_prop(ctx, node, "title")?;
    let height = opt_u16_prop(ctx, node, "height")?;
    let mut entries = Vec::new();
    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        if child.name().value() != "entry" {
            return Err(ctx.err(
                child.span(),
                format!("unknown child `{}` in `log`", child.name().value()),
                "expected `entry`",
                None,
            ));
        }
        entries.push(parse_log_entry(ctx, child)?);
    }
    Ok(Node::Log {
        id,
        title,
        height,
        entries,
    })
}

fn parse_log_entry(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<LogEntry> {
    let ts = opt_str_prop(ctx, node, "ts")?;
    let level = match opt_str_prop(ctx, node, "level")? {
        Some(raw) => LogLevel::parse(&raw).ok_or_else(|| {
            ctx.err(
                prop_span(node, "level"),
                format!("invalid log level `{raw}`"),
                "expected trace, debug, info, warn, or error",
                None,
            )
        })?,
        None => LogLevel::Info,
    };
    let source = opt_str_prop(ctx, node, "source")?;
    let message = opt_str_prop_or_arg(ctx, node, "message", 0)?.ok_or_else(|| {
        ctx.err(
            node.span(),
            "missing message on log `entry`",
            "add message arg or `message=\"...\"`",
            None,
        )
    })?;
    Ok(LogEntry {
        ts,
        level,
        source,
        message,
    })
}

fn parse_graph(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = req_id(ctx, node, "graph")?;
    let title = opt_str_prop(ctx, node, "title")?;
    let caption = opt_str_prop(ctx, node, "caption")?;
    let height = opt_u16_prop(ctx, node, "height")?;
    let direction = match opt_str_prop(ctx, node, "direction")? {
        Some(raw) => GraphDirection::parse(&raw).ok_or_else(|| {
            ctx.err(
                prop_span(node, "direction"),
                format!("invalid graph direction `{raw}`"),
                "expected `lr` or `tb`",
                None,
            )
        })?,
        None => GraphDirection::Lr,
    };
    let highlight_key = opt_str_prop(ctx, node, "highlight-key")?;
    let select_key = opt_str_prop(ctx, node, "select-key")?;

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        match child.name().value() {
            "node" => {
                let nid = opt_str_prop_or_arg(ctx, child, "id", 0)?.ok_or_else(|| {
                    ctx.err(
                        child.span(),
                        "missing required `id` on graph `node`",
                        "add node id",
                        None,
                    )
                })?;
                let label =
                    opt_str_prop_or_arg(ctx, child, "label", 1)?.unwrap_or_else(|| nid.clone());
                let sublabel = opt_str_prop(ctx, child, "sublabel")?;
                let status = opt_status_prop(ctx, child, "status")?;
                let group = opt_str_prop(ctx, child, "group")?;
                let shape = match opt_str_prop(ctx, child, "shape")? {
                    Some(raw) => NodeShape::parse(&raw).ok_or_else(|| {
                        ctx.err(
                            prop_span(child, "shape"),
                            format!("invalid node shape `{raw}`"),
                            "expected rounded, box, diamond, or pill",
                            None,
                        )
                    })?,
                    None => NodeShape::Rounded,
                };
                let tooltip = opt_str_prop(ctx, child, "tooltip")?;
                nodes.push(GraphNode {
                    id: nid,
                    label,
                    sublabel,
                    status,
                    group,
                    shape,
                    tooltip,
                });
            }
            "edge" => {
                let from = opt_str_prop_or_arg(ctx, child, "from", 0)?.ok_or_else(|| {
                    ctx.err(
                        child.span(),
                        "missing required `from` on graph `edge`",
                        "add source node id",
                        None,
                    )
                })?;
                let to = opt_str_prop_or_arg(ctx, child, "to", 1)?.ok_or_else(|| {
                    ctx.err(
                        child.span(),
                        "missing required `to` on graph `edge`",
                        "add target node id",
                        None,
                    )
                })?;
                let label = opt_str_prop(ctx, child, "label")?;
                let style = match opt_str_prop(ctx, child, "style")? {
                    Some(raw) => EdgeStyle::parse(&raw).ok_or_else(|| {
                        ctx.err(
                            prop_span(child, "style"),
                            format!("invalid edge style `{raw}`"),
                            "expected `solid` or `dashed`",
                            None,
                        )
                    })?,
                    None => EdgeStyle::Solid,
                };
                let active = opt_bool_prop(ctx, child, "active")?.unwrap_or(false);
                let tooltip = opt_str_prop(ctx, child, "tooltip")?;
                edges.push(GraphEdge {
                    from,
                    to,
                    label,
                    style,
                    active,
                    tooltip,
                });
            }
            other => {
                return Err(ctx.err(
                    child.span(),
                    format!("unknown child `{other}` in `graph`"),
                    "expected `node` or `edge`",
                    None,
                ));
            }
        }
    }

    Ok(Node::Graph {
        id,
        title,
        caption,
        height,
        direction,
        nodes,
        edges,
        highlight_key,
        select_key,
    })
}

fn parse_chart(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = req_id(ctx, node, "chart")?;
    let title = opt_str_prop(ctx, node, "title")?;
    let caption = opt_str_prop(ctx, node, "caption")?;
    let height = opt_u16_prop(ctx, node, "height")?;
    let chart_type = match opt_str_prop(ctx, node, "type")? {
        Some(raw) => ChartType::parse(&raw).ok_or_else(|| {
            ctx.err(
                prop_span(node, "type"),
                format!("invalid chart type `{raw}`"),
                "expected line, area, bar, hbar, or scatter",
                None,
            )
        })?,
        None => ChartType::Line,
    };
    let x_labels = match opt_str_prop(ctx, node, "x-labels")? {
        Some(raw) => split_csv(&raw),
        None => Vec::new(),
    };
    let x_unit = opt_str_prop(ctx, node, "x-unit")?;
    let y_unit = opt_str_prop(ctx, node, "y-unit")?;
    let value_labels = opt_bool_prop(ctx, node, "value-labels")?.unwrap_or(false);
    let scale = opt_str_prop(ctx, node, "scale")?;

    let mut series = Vec::new();
    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        if child.name().value() != "series" {
            return Err(ctx.err(
                child.span(),
                format!("unknown child `{}` in `chart`", child.name().value()),
                "expected `series`",
                None,
            ));
        }
        let name = opt_str_prop_or_arg(ctx, child, "name", 0)?.ok_or_else(|| {
            ctx.err(
                child.span(),
                "missing required `name` on chart `series`",
                "add series name",
                None,
            )
        })?;
        let color = opt_str_prop(ctx, child, "color")?;
        let mut data = Vec::new();
        if let Some(raw) = opt_str_prop(ctx, child, "data")? {
            for y in parse_f64_csv(ctx, prop_span(child, "data"), &raw)? {
                data.push(ChartPoint { x: None, y });
            }
        }
        for entry in positional_entries(child).skip(1) {
            let y = val_as_f64(ctx, entry.span(), entry.value(), "point")?;
            data.push(ChartPoint { x: None, y });
        }
        for pt_node in child.children().map(|d| d.nodes()).unwrap_or(&[]) {
            if pt_node.name().value() != "point" {
                return Err(ctx.err(
                    pt_node.span(),
                    format!("unknown child `{}` in `series`", pt_node.name().value()),
                    "expected `point`",
                    None,
                ));
            }
            data.push(parse_chart_point(ctx, pt_node)?);
        }
        series.push(ChartSeries { name, color, data });
    }

    Ok(Node::Chart {
        id,
        title,
        caption,
        height,
        chart_type,
        x_labels,
        x_unit,
        y_unit,
        value_labels,
        scale,
        series,
    })
}

fn parse_chart_point(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<ChartPoint> {
    let x = opt_f64_prop(ctx, node, "x")?;
    let y = if let Some(y_val) = opt_f64_prop(ctx, node, "y")? {
        y_val
    } else if let Some(entry) = positional_entries(node).next() {
        val_as_f64(ctx, entry.span(), entry.value(), "y")?
    } else {
        return Err(ctx.err(
            node.span(),
            "missing `y` value on `point`",
            "expected `point 42.0` or `point x=1.0 y=42.0`",
            None,
        ));
    };
    Ok(ChartPoint { x, y })
}

fn parse_svg(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Node> {
    let id = req_id(ctx, node, "svg")?;
    let title = opt_str_prop(ctx, node, "title")?;
    let caption = opt_str_prop(ctx, node, "caption")?;
    let height = opt_u16_prop(ctx, node, "height")?;
    let select_key = opt_str_prop(ctx, node, "select-key")?;
    let highlight_key = opt_str_prop(ctx, node, "highlight-key")?;
    let mut source = opt_str_arg(ctx, node, 0)?.unwrap_or_default();
    let mut tooltips = BTreeMap::new();

    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
        match child.name().value() {
            "source" => {
                source = req_str_arg_or_prop(ctx, child, 0, "text")?;
            }
            "tooltip" => {
                let (k, v) = parse_tooltip_node(ctx, child)?;
                tooltips.insert(k, v);
            }
            "tooltips" => {
                for sub in child.children().map(|d| d.nodes()).unwrap_or(&[]) {
                    if sub.name().value() != "tooltip" {
                        return Err(ctx.err(
                            sub.span(),
                            format!("unknown child `{}` in `tooltips`", sub.name().value()),
                            "expected `tooltip`",
                            None,
                        ));
                    }
                    let (k, v) = parse_tooltip_node(ctx, sub)?;
                    tooltips.insert(k, v);
                }
            }
            other => {
                return Err(ctx.err(
                    child.span(),
                    format!("unknown child `{other}` in `svg`"),
                    "expected `source` or `tooltip`",
                    None,
                ));
            }
        }
    }

    if source.is_empty() {
        return Err(ctx.err(
            node.span(),
            "missing SVG source on `svg` node",
            "provide positional SVG string or `source` child",
            None,
        ));
    }

    Ok(Node::Svg {
        id,
        title,
        caption,
        height,
        source,
        tooltips,
        select_key,
        highlight_key,
    })
}

fn parse_tooltip_node(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<(String, String)> {
    let target = opt_str_prop_or_arg(ctx, node, "id", 0)?.ok_or_else(|| {
        ctx.err(
            node.span(),
            "missing target id on `tooltip`",
            "expected `tooltip \"id\" \"text\"`",
            None,
        )
    })?;
    let text = opt_str_prop_or_arg(ctx, node, "text", 1)?.ok_or_else(|| {
        ctx.err(
            node.span(),
            "missing text on `tooltip`",
            "expected `tooltip \"id\" \"text\"`",
            None,
        )
    })?;
    Ok((target, text))
}

fn parse_event_node(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Event> {
    match node.name().value() {
        "viz" => Ok(Event::Init(Box::new(parse_viz_node(ctx, node)?))),
        "set-state" => {
            let mut state = StateMap::new();
            parse_state_entries(ctx, node, &mut state, &[])?;
            Ok(Event::SetState(state))
        }
        "set-node" => {
            let graph_id = opt_str_prop(ctx, node, "graph")?
                .or(opt_str_prop(ctx, node, "graph-id")?)
                .or(opt_str_arg(ctx, node, 0)?)
                .ok_or_else(|| {
                    ctx.err(
                        node.span(),
                        "missing `graph` on `set-node`",
                        "add `graph=\"...\"` or positional arg",
                        None,
                    )
                })?;
            let node_id = opt_str_prop(ctx, node, "node")?
                .or(opt_str_prop(ctx, node, "node-id")?)
                .or(opt_str_arg(ctx, node, 1)?)
                .ok_or_else(|| {
                    ctx.err(
                        node.span(),
                        "missing `node` on `set-node`",
                        "add `node=\"...\"` or positional arg",
                        None,
                    )
                })?;
            Ok(Event::SetNode {
                graph_id,
                node_id,
                label: opt_str_prop(ctx, node, "label")?,
                sublabel: opt_str_prop(ctx, node, "sublabel")?,
                status: opt_status_prop(ctx, node, "status")?,
                tooltip: opt_str_prop(ctx, node, "tooltip")?,
            })
        }
        "set-edge" => {
            let graph_id = opt_str_prop(ctx, node, "graph")?
                .or(opt_str_prop(ctx, node, "graph-id")?)
                .or(opt_str_arg(ctx, node, 0)?)
                .ok_or_else(|| {
                    ctx.err(
                        node.span(),
                        "missing `graph` on `set-edge`",
                        "add `graph=\"...\"`",
                        None,
                    )
                })?;
            let from = opt_str_prop_or_arg(ctx, node, "from", 1)?.ok_or_else(|| {
                ctx.err(
                    node.span(),
                    "missing `from` on `set-edge`",
                    "add `from=\"...\"`",
                    None,
                )
            })?;
            let to = opt_str_prop_or_arg(ctx, node, "to", 2)?.ok_or_else(|| {
                ctx.err(
                    node.span(),
                    "missing `to` on `set-edge`",
                    "add `to=\"...\"`",
                    None,
                )
            })?;
            Ok(Event::SetEdge {
                graph_id,
                from,
                to,
                label: opt_str_prop(ctx, node, "label")?,
                active: opt_bool_prop(ctx, node, "active")?,
                tooltip: opt_str_prop(ctx, node, "tooltip")?,
            })
        }
        "set-stat" => {
            let stat_id = opt_str_prop(ctx, node, "stat")?
                .or(opt_str_prop(ctx, node, "stat-id")?)
                .or(opt_str_arg(ctx, node, 0)?)
                .ok_or_else(|| {
                    ctx.err(
                        node.span(),
                        "missing `stat` on `set-stat`",
                        "add `stat=\"...\"`",
                        None,
                    )
                })?;
            let item_id = opt_str_prop(ctx, node, "item")?
                .or(opt_str_prop(ctx, node, "item-id")?)
                .or(opt_str_arg(ctx, node, 1)?)
                .ok_or_else(|| {
                    ctx.err(
                        node.span(),
                        "missing `item` on `set-stat`",
                        "add `item=\"...\"`",
                        None,
                    )
                })?;
            Ok(Event::SetStat {
                stat_id,
                item_id,
                value: opt_str_prop(ctx, node, "value")?,
                delta: opt_str_prop(ctx, node, "delta")?,
                status: opt_status_prop(ctx, node, "status")?,
                sparkline_push: opt_f64_prop(ctx, node, "sparkline-push")?,
            })
        }
        "log-append" => {
            let target = opt_str_prop_or_arg(ctx, node, "target", 0)?.ok_or_else(|| {
                ctx.err(
                    node.span(),
                    "missing `target` on `log-append`",
                    "add `target=\"...\"`",
                    None,
                )
            })?;
            let mut entries = Vec::new();
            for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
                if child.name().value() != "entry" {
                    return Err(ctx.err(
                        child.span(),
                        format!("unknown child `{}` in `log-append`", child.name().value()),
                        "expected `entry`",
                        None,
                    ));
                }
                entries.push(parse_log_entry(ctx, child)?);
            }
            Ok(Event::LogAppend { target, entries })
        }
        "chart-append" => {
            let mut pos_cursor = 0usize;
            let target = match opt_str_prop(ctx, node, "target")? {
                Some(v) => v,
                None => {
                    let v = opt_str_arg(ctx, node, pos_cursor)?.ok_or_else(|| {
                        ctx.err(
                            node.span(),
                            "missing `target` on `chart-append`",
                            "add `target=\"...\"`",
                            None,
                        )
                    })?;
                    pos_cursor += 1;
                    v
                }
            };
            let series = match opt_str_prop(ctx, node, "series")? {
                Some(v) => v,
                None => {
                    let v = opt_str_arg(ctx, node, pos_cursor)?.ok_or_else(|| {
                        ctx.err(
                            node.span(),
                            "missing `series` on `chart-append`",
                            "add `series=\"...\"`",
                            None,
                        )
                    })?;
                    pos_cursor += 1;
                    v
                }
            };
            let x_label = opt_str_prop(ctx, node, "x-label")?;
            let max_points = opt_usize_prop(ctx, node, "max-points")?;
            let mut points = Vec::new();
            for entry in positional_entries(node).skip(pos_cursor) {
                let y = val_as_f64(ctx, entry.span(), entry.value(), "point")?;
                points.push(ChartPoint { x: None, y });
            }
            if let Some(y) = opt_f64_prop(ctx, node, "y")? {
                points.push(ChartPoint {
                    x: opt_f64_prop(ctx, node, "x")?,
                    y,
                });
            }
            if let Some(raw) = opt_str_prop(ctx, node, "data")? {
                for y in parse_f64_csv(ctx, prop_span(node, "data"), &raw)? {
                    points.push(ChartPoint { x: None, y });
                }
            }
            for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
                if child.name().value() != "point" {
                    return Err(ctx.err(
                        child.span(),
                        format!("unknown child `{}` in `chart-append`", child.name().value()),
                        "expected `point`",
                        None,
                    ));
                }
                points.push(parse_chart_point(ctx, child)?);
            }
            Ok(Event::ChartAppend {
                target,
                series,
                x_label,
                points,
                max_points,
            })
        }
        "replace" => {
            let target = opt_str_prop_or_arg(ctx, node, "target", 0)?.ok_or_else(|| {
                ctx.err(
                    node.span(),
                    "missing `target` on `replace`",
                    "add `target=\"...\"`",
                    None,
                )
            })?;
            let children = node.children().map(|d| d.nodes()).unwrap_or(&[]);
            if children.len() != 1 {
                return Err(ctx.err(
                    node.span(),
                    "`replace` must contain exactly one replacement node",
                    "expected one child node",
                    None,
                ));
            }
            Ok(Event::Replace {
                target,
                node: Box::new(parse_node(ctx, &children[0])?),
            })
        }
        "close" => Ok(Event::Close),
        other => Err(ctx.err(
            node.span(),
            format!("unknown event op `{other}`"),
            "unsupported event",
            Some(
                "Valid events: viz, set-state, set-node, set-edge, set-stat, log-append, chart-append, replace, close."
                    .into(),
            ),
        )),
    }
}

fn req_id(ctx: &ParserCtx<'_>, node: &KdlNode, kind: &str) -> miette::Result<String> {
    opt_str_prop(ctx, node, "id")?.ok_or_else(|| {
        ctx.err(
            node.span(),
            format!("missing required `id` on `{kind}`"),
            "add `id=\"...\"`",
            None,
        )
    })
}

fn find_child<'a>(node: &'a KdlNode, name: &str) -> Option<&'a KdlNode> {
    node.children()?
        .nodes()
        .iter()
        .find(|n| n.name().value() == name)
}

fn positional_entries(node: &KdlNode) -> impl Iterator<Item = &KdlEntry> {
    node.entries().iter().filter(|e| e.name().is_none())
}

fn positional_strings(ctx: &ParserCtx<'_>, node: &KdlNode) -> miette::Result<Vec<String>> {
    positional_entries(node)
        .map(|e| val_as_string(ctx, e.span(), e.value(), "arg"))
        .collect()
}

fn prop_span(node: &KdlNode, key: &str) -> SourceSpan {
    node.entry(key)
        .map(|e| e.span())
        .unwrap_or_else(|| node.span())
}

fn prop_or_arg_span(node: &KdlNode, key: &str, arg_idx: usize) -> SourceSpan {
    if let Some(e) = node.entry(key) {
        return e.span();
    }
    positional_entries(node)
        .nth(arg_idx)
        .map(|e| e.span())
        .unwrap_or_else(|| node.span())
}

fn opt_str_arg(ctx: &ParserCtx<'_>, node: &KdlNode, idx: usize) -> miette::Result<Option<String>> {
    match positional_entries(node).nth(idx) {
        Some(e) => Ok(Some(val_as_string(ctx, e.span(), e.value(), "arg")?)),
        None => Ok(None),
    }
}

fn opt_str_prop(ctx: &ParserCtx<'_>, node: &KdlNode, key: &str) -> miette::Result<Option<String>> {
    match node.entry(key) {
        Some(e) => Ok(Some(val_as_string(ctx, e.span(), e.value(), key)?)),
        None => Ok(None),
    }
}

fn opt_str_prop_or_arg(
    ctx: &ParserCtx<'_>,
    node: &KdlNode,
    key: &str,
    arg_idx: usize,
) -> miette::Result<Option<String>> {
    if let Some(v) = opt_str_prop(ctx, node, key)? {
        return Ok(Some(v));
    }
    opt_str_arg(ctx, node, arg_idx)
}

fn req_str_arg_or_prop(
    ctx: &ParserCtx<'_>,
    node: &KdlNode,
    arg_idx: usize,
    key: &str,
) -> miette::Result<String> {
    opt_str_prop_or_arg(ctx, node, key, arg_idx)?.ok_or_else(|| {
        ctx.err(
            node.span(),
            format!("missing string content on `{}`", node.name().value()),
            "expected string argument",
            None,
        )
    })
}

fn opt_bool_prop(ctx: &ParserCtx<'_>, node: &KdlNode, key: &str) -> miette::Result<Option<bool>> {
    let Some(entry) = node.entry(key) else {
        return Ok(None);
    };
    match entry.value() {
        KdlValue::Bool(b) => Ok(Some(*b)),
        _ => Err(ctx.err(
            entry.span(),
            format!("expected boolean for `{key}`"),
            "expected `#true` or `#false`",
            None,
        )),
    }
}

fn opt_f64_prop(ctx: &ParserCtx<'_>, node: &KdlNode, key: &str) -> miette::Result<Option<f64>> {
    let Some(entry) = node.entry(key) else {
        return Ok(None);
    };
    Ok(Some(val_as_f64(ctx, entry.span(), entry.value(), key)?))
}

fn opt_usize_prop(ctx: &ParserCtx<'_>, node: &KdlNode, key: &str) -> miette::Result<Option<usize>> {
    let Some(entry) = node.entry(key) else {
        return Ok(None);
    };
    match entry.value() {
        KdlValue::Integer(i) => usize::try_from(*i).map(Some).map_err(|_| {
            ctx.err(
                entry.span(),
                format!("expected non-negative integer for `{key}`"),
                "invalid integer",
                None,
            )
        }),
        _ => Err(ctx.err(
            entry.span(),
            format!("expected integer for `{key}`"),
            "expected integer",
            None,
        )),
    }
}

fn opt_status_prop(
    ctx: &ParserCtx<'_>,
    node: &KdlNode,
    key: &str,
) -> miette::Result<Option<Status>> {
    match opt_str_prop(ctx, node, key)? {
        Some(raw) => Status::parse(&raw).map(Some).ok_or_else(|| {
            ctx.err(
                prop_span(node, key),
                format!("invalid status `{raw}`"),
                "expected ok, warn, error, active, idle, or muted",
                None,
            )
        }),
        None => Ok(None),
    }
}

fn opt_u16_prop(ctx: &ParserCtx<'_>, node: &KdlNode, key: &str) -> miette::Result<Option<u16>> {
    let Some(entry) = node.entry(key) else {
        return Ok(None);
    };
    match entry.value() {
        KdlValue::Integer(i) => u16::try_from(*i).map(Some).map_err(|_| {
            ctx.err(
                entry.span(),
                format!("expected 0..=65535 integer for `{key}`"),
                "invalid height",
                None,
            )
        }),
        _ => Err(ctx.err(
            entry.span(),
            format!("expected integer for `{key}`"),
            "expected integer",
            None,
        )),
    }
}

fn val_as_string(
    ctx: &ParserCtx<'_>,
    span: SourceSpan,
    val: &KdlValue,
    key: &str,
) -> miette::Result<String> {
    match val {
        KdlValue::String(s) => Ok(s.clone()),
        _ => Err(ctx.err(
            span,
            format!("expected string for `{key}`"),
            "expected string value",
            None,
        )),
    }
}

fn val_as_f64(
    ctx: &ParserCtx<'_>,
    span: SourceSpan,
    val: &KdlValue,
    key: &str,
) -> miette::Result<f64> {
    match val {
        KdlValue::Float(f) => Ok(*f),
        KdlValue::Integer(i) => Ok(*i as f64),
        _ => Err(ctx.err(
            span,
            format!("expected number for `{key}`"),
            "expected numeric value",
            None,
        )),
    }
}

fn val_as_u32(
    ctx: &ParserCtx<'_>,
    span: SourceSpan,
    val: &KdlValue,
    key: &str,
) -> miette::Result<u32> {
    match val {
        KdlValue::Integer(i) => u32::try_from(*i).map_err(|_| {
            ctx.err(
                span,
                format!("integer out of u32 range for `{key}`"),
                "invalid integer",
                None,
            )
        }),
        _ => Err(ctx.err(
            span,
            format!("expected integer for `{key}`"),
            "expected integer",
            None,
        )),
    }
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_u16_csv(ctx: &ParserCtx<'_>, span: SourceSpan, raw: &str) -> miette::Result<Vec<u16>> {
    split_csv(raw)
        .into_iter()
        .map(|part| {
            part.parse::<u16>().map_err(|_| {
                ctx.err(
                    span,
                    format!("invalid weight `{part}`"),
                    "expected comma-separated positive integers",
                    None,
                )
            })
        })
        .collect()
}

fn parse_f64_csv(ctx: &ParserCtx<'_>, span: SourceSpan, raw: &str) -> miette::Result<Vec<f64>> {
    split_csv(raw)
        .into_iter()
        .map(|part| {
            part.parse::<f64>().map_err(|_| {
                ctx.err(
                    span,
                    format!("invalid numeric value `{part}`"),
                    "expected comma-separated numbers",
                    None,
                )
            })
        })
        .collect()
}

fn parse_highlight_ranges(
    ctx: &ParserCtx<'_>,
    span: SourceSpan,
    raw: &str,
) -> miette::Result<Vec<(usize, usize)>> {
    split_csv(raw)
        .into_iter()
        .map(|part| {
            if let Some(range) = parse_line_range(&part) {
                Ok(range)
            } else if let Ok(line) = part.parse::<usize>() {
                Ok((line, line))
            } else {
                Err(ctx.err(
                    span,
                    format!("invalid highlight range `{part}`"),
                    "expected `start..end` or line number",
                    None,
                ))
            }
        })
        .collect()
}

pub fn format_document_kdl(doc: &Document) -> String {
    let mut out = String::new();
    write_document(&mut out, doc, 0);
    out
}

pub fn format_node_kdl(node: &Node) -> String {
    let mut out = String::new();
    write_node(&mut out, node, 0);
    out
}

pub fn format_event_kdl(event: &Event) -> String {
    let mut out = String::new();
    match event {
        Event::Init(doc) => write_document(&mut out, doc, 0),
        Event::SetState(state) => {
            out.push_str("set-state");
            write_state_props(&mut out, state);
            out.push('\n');
        }
        Event::SetNode {
            graph_id,
            node_id,
            label,
            sublabel,
            status,
            tooltip,
        } => {
            let _ = write!(
                out,
                "set-node graph={} node={}",
                fmt_str(graph_id),
                fmt_str(node_id)
            );
            write_opt_str_prop(&mut out, "label", label.as_deref());
            write_opt_str_prop(&mut out, "sublabel", sublabel.as_deref());
            if let Some(st) = status {
                let _ = write!(out, " status={}", fmt_str(st.as_str()));
            }
            write_opt_str_prop(&mut out, "tooltip", tooltip.as_deref());
            out.push('\n');
        }
        Event::SetEdge {
            graph_id,
            from,
            to,
            label,
            active,
            tooltip,
        } => {
            let _ = write!(
                out,
                "set-edge graph={} from={} to={}",
                fmt_str(graph_id),
                fmt_str(from),
                fmt_str(to)
            );
            write_opt_str_prop(&mut out, "label", label.as_deref());
            if let Some(a) = active {
                let _ = write!(out, " active=#{a}");
            }
            write_opt_str_prop(&mut out, "tooltip", tooltip.as_deref());
            out.push('\n');
        }
        Event::SetStat {
            stat_id,
            item_id,
            value,
            delta,
            status,
            sparkline_push,
        } => {
            let _ = write!(
                out,
                "set-stat stat={} item={}",
                fmt_str(stat_id),
                fmt_str(item_id)
            );
            write_opt_str_prop(&mut out, "value", value.as_deref());
            write_opt_str_prop(&mut out, "delta", delta.as_deref());
            if let Some(st) = status {
                let _ = write!(out, " status={}", fmt_str(st.as_str()));
            }
            if let Some(v) = sparkline_push {
                let _ = write!(out, " sparkline-push={}", fmt_f64(*v));
            }
            out.push('\n');
        }
        Event::LogAppend { target, entries } => {
            let _ = writeln!(out, "log-append target={} {{", fmt_str(target));
            for entry in entries {
                write_log_entry(&mut out, entry, 1);
            }
            out.push_str("}\n");
        }
        Event::ChartAppend {
            target,
            series,
            x_label,
            points,
            max_points,
        } => {
            let _ = write!(
                out,
                "chart-append target={} series={}",
                fmt_str(target),
                fmt_str(series)
            );
            write_opt_str_prop(&mut out, "x-label", x_label.as_deref());
            if let Some(max) = max_points {
                let _ = write!(out, " max-points={max}");
            }
            if points.is_empty() {
                out.push('\n');
            } else {
                out.push_str(" {\n");
                for pt in points {
                    write_chart_point(&mut out, pt, 1);
                }
                out.push_str("}\n");
            }
        }
        Event::Replace { target, node } => {
            let _ = writeln!(out, "replace target={} {{", fmt_str(target));
            write_node(&mut out, node, 1);
            out.push_str("}\n");
        }
        Event::Close => out.push_str("close\n"),
    }
    out
}

fn write_document(out: &mut String, doc: &Document, depth: usize) {
    push_indent(out, depth);
    let _ = write!(out, "viz v={} title={}", doc.v, fmt_str(&doc.title));
    write_opt_str_prop(out, "subtitle", doc.subtitle.as_deref());
    if let Some(w) = doc.width {
        let _ = write!(out, " width={}", fmt_f64(w));
    }
    if let Some(h) = doc.height {
        let _ = write!(out, " height={}", fmt_f64(h));
    }
    out.push_str(" {\n");

    if !doc.state.is_empty() {
        push_indent(out, depth + 1);
        out.push_str("state");
        write_state_props(out, &doc.state);
        out.push('\n');
    }

    if !doc.steps.is_empty() {
        push_indent(out, depth + 1);
        out.push_str("steps {\n");
        for step in &doc.steps {
            push_indent(out, depth + 2);
            let _ = write!(out, "step {}", fmt_str(&step.title));
            write_opt_str_prop(out, "id", step.id.as_deref());
            write_state_props(out, &step.state);
            if step.body.is_empty() {
                out.push('\n');
            } else {
                out.push_str(" {\n");
                push_indent(out, depth + 3);
                out.push_str("markdown ");
                write_string_arg(out, &step.body, depth + 3);
                out.push('\n');
                push_indent(out, depth + 2);
                out.push_str("}\n");
            }
        }
        push_indent(out, depth + 1);
        out.push_str("}\n");
    }

    write_node(out, &doc.root, depth + 1);
    push_indent(out, depth);
    out.push_str("}\n");
}

fn write_node(out: &mut String, node: &Node, depth: usize) {
    match node {
        Node::Split {
            direction,
            ratio,
            weights,
            children,
        } => {
            push_indent(out, depth);
            let _ = write!(out, "split direction={}", fmt_str(direction.as_str()));
            if let Some(r) = ratio {
                let _ = write!(out, " ratio={}", fmt_f64(f64::from(*r)));
            }
            if !weights.is_empty() {
                let joined = weights
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                let _ = write!(out, " weights={}", fmt_str(&joined));
            }
            out.push_str(" {\n");
            for child in children {
                write_node(out, child, depth + 1);
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Scroll { id, children } => {
            push_indent(out, depth);
            let _ = writeln!(out, "scroll id={} {{", fmt_str(id));
            for child in children {
                write_node(out, child, depth + 1);
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Tabs { id, items } => {
            push_indent(out, depth);
            let _ = writeln!(out, "tabs id={} {{", fmt_str(id));
            for item in items {
                push_indent(out, depth + 1);
                let _ = write!(out, "tab {}", fmt_str(&item.title));
                write_opt_str_prop(out, "id", item.id.as_deref());
                out.push_str(" {\n");
                write_node(out, &item.node, depth + 2);
                push_indent(out, depth + 1);
                out.push_str("}\n");
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Markdown {
            id,
            title,
            tone,
            height,
            content,
            by_selected,
        } => {
            push_indent(out, depth);
            out.push_str("markdown");
            write_opt_str_prop(out, "id", id.as_deref());
            write_opt_str_prop(out, "title", title.as_deref());
            if let Some(st) = tone {
                let _ = write!(out, " tone={}", fmt_str(st.as_str()));
            }
            write_opt_u16_prop(out, "height", *height);
            if by_selected.is_empty() {
                out.push(' ');
                write_string_arg(out, content, depth);
                out.push('\n');
            } else {
                out.push_str(" {\n");
                push_indent(out, depth + 1);
                out.push_str("content ");
                write_string_arg(out, content, depth + 1);
                out.push('\n');
                push_indent(out, depth + 1);
                out.push_str("by-selected {\n");
                for (k, v) in by_selected {
                    push_indent(out, depth + 2);
                    let _ = write!(out, "case {} ", fmt_str(k));
                    write_string_arg(out, v, depth + 2);
                    out.push('\n');
                }
                push_indent(out, depth + 1);
                out.push_str("}\n");
                push_indent(out, depth);
                out.push_str("}\n");
            }
        }
        Node::Code {
            id,
            title,
            lang,
            path,
            lines,
            height,
            source,
            start_line,
            highlight_lines,
            focus_key,
            by_selected,
        } => {
            push_indent(out, depth);
            out.push_str("code");
            write_opt_str_prop(out, "id", id.as_deref());
            write_opt_str_prop(out, "title", title.as_deref());
            write_opt_str_prop(out, "lang", lang.as_deref());
            write_opt_str_prop(out, "path", path.as_deref());
            if let Some((lo, hi)) = lines {
                let _ = write!(out, " lines={}", fmt_str(&format!("{lo}..{hi}")));
            }
            write_opt_u16_prop(out, "height", *height);
            if *start_line != 1 {
                let _ = write!(out, " start-line={start_line}");
            }
            if !highlight_lines.is_empty() {
                let _ = write!(
                    out,
                    " highlight-lines={}",
                    fmt_str(&fmt_ranges(highlight_lines))
                );
            }
            write_opt_str_prop(out, "focus-key", focus_key.as_deref());
            let has_body = (!source.is_empty() || path.is_none()) || !by_selected.is_empty();
            if !has_body {
                out.push('\n');
                return;
            }
            out.push_str(" {\n");
            if !source.is_empty() || path.is_none() {
                push_indent(out, depth + 1);
                out.push_str("source ");
                write_string_arg(out, source, depth + 1);
                out.push('\n');
            }
            if !by_selected.is_empty() {
                push_indent(out, depth + 1);
                out.push_str("by-selected {\n");
                for (k, variant) in by_selected {
                    push_indent(out, depth + 2);
                    let _ = write!(out, "case {}", fmt_str(k));
                    write_opt_str_prop(out, "lang", variant.lang.as_deref());
                    write_opt_str_prop(out, "path", variant.path.as_deref());
                    if let Some((lo, hi)) = variant.lines {
                        let _ = write!(out, " lines={}", fmt_str(&format!("{lo}..{hi}")));
                    }
                    if variant.start_line != 1 {
                        let _ = write!(out, " start-line={}", variant.start_line);
                    }
                    if !variant.highlight_lines.is_empty() {
                        let _ = write!(
                            out,
                            " highlight-lines={}",
                            fmt_str(&fmt_ranges(&variant.highlight_lines))
                        );
                    }
                    if variant.source.is_empty() && variant.path.is_some() {
                        out.push('\n');
                    } else {
                        out.push_str(" {\n");
                        push_indent(out, depth + 3);
                        out.push_str("source ");
                        write_string_arg(out, &variant.source, depth + 3);
                        out.push('\n');
                        push_indent(out, depth + 2);
                        out.push_str("}\n");
                    }
                }
                push_indent(out, depth + 1);
                out.push_str("}\n");
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Stat {
            id,
            title,
            height,
            items,
        } => {
            push_indent(out, depth);
            out.push_str("stat");
            write_opt_str_prop(out, "id", id.as_deref());
            write_opt_str_prop(out, "title", title.as_deref());
            write_opt_u16_prop(out, "height", *height);
            out.push_str(" {\n");
            for item in items {
                push_indent(out, depth + 1);
                let _ = write!(
                    out,
                    "item {} label={} value={}",
                    fmt_str(&item.id),
                    fmt_str(&item.label),
                    fmt_str(&item.value)
                );
                write_opt_str_prop(out, "delta", item.delta.as_deref());
                if let Some(st) = item.status {
                    let _ = write!(out, " status={}", fmt_str(st.as_str()));
                }
                if !item.sparkline.is_empty() {
                    let joined = item
                        .sparkline
                        .iter()
                        .map(|v| fmt_f64(*v))
                        .collect::<Vec<_>>()
                        .join(",");
                    let _ = write!(out, " sparkline={}", fmt_str(&joined));
                }
                out.push('\n');
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Table {
            id,
            title,
            height,
            columns,
            rows,
        } => {
            push_indent(out, depth);
            out.push_str("table");
            write_opt_str_prop(out, "id", id.as_deref());
            write_opt_str_prop(out, "title", title.as_deref());
            write_opt_u16_prop(out, "height", *height);
            out.push_str(" {\n");
            if !columns.is_empty() {
                push_indent(out, depth + 1);
                out.push_str("columns");
                for col in columns {
                    let _ = write!(out, " {}", fmt_str(col));
                }
                out.push('\n');
            }
            for row in rows {
                push_indent(out, depth + 1);
                out.push_str("row");
                write_opt_str_prop(out, "id", row.id.as_deref());
                if let Some(st) = row.status {
                    let _ = write!(out, " status={}", fmt_str(st.as_str()));
                }
                for cell in &row.cells {
                    let _ = write!(out, " {}", fmt_str(cell));
                }
                out.push('\n');
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Log {
            id,
            title,
            height,
            entries,
        } => {
            push_indent(out, depth);
            let _ = write!(out, "log id={}", fmt_str(id));
            write_opt_str_prop(out, "title", title.as_deref());
            write_opt_u16_prop(out, "height", *height);
            out.push_str(" {\n");
            for entry in entries {
                write_log_entry(out, entry, depth + 1);
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Graph {
            id,
            title,
            caption,
            height,
            direction,
            nodes,
            edges,
            highlight_key,
            select_key,
        } => {
            push_indent(out, depth);
            let _ = write!(
                out,
                "graph id={} direction={}",
                fmt_str(id),
                fmt_str(direction.as_str())
            );
            write_opt_str_prop(out, "title", title.as_deref());
            write_opt_str_prop(out, "caption", caption.as_deref());
            write_opt_u16_prop(out, "height", *height);
            write_opt_str_prop(out, "highlight-key", highlight_key.as_deref());
            write_opt_str_prop(out, "select-key", select_key.as_deref());
            out.push_str(" {\n");
            for gnode in nodes {
                push_indent(out, depth + 1);
                let _ = write!(
                    out,
                    "node {} label={}",
                    fmt_str(&gnode.id),
                    fmt_str(&gnode.label)
                );
                write_opt_str_prop(out, "sublabel", gnode.sublabel.as_deref());
                if let Some(st) = gnode.status {
                    let _ = write!(out, " status={}", fmt_str(st.as_str()));
                }
                write_opt_str_prop(out, "group", gnode.group.as_deref());
                if gnode.shape != NodeShape::Rounded {
                    let _ = write!(out, " shape={}", fmt_str(gnode.shape.as_str()));
                }
                write_opt_str_prop(out, "tooltip", gnode.tooltip.as_deref());
                out.push('\n');
            }
            for edge in edges {
                push_indent(out, depth + 1);
                let _ = write!(out, "edge {} {}", fmt_str(&edge.from), fmt_str(&edge.to));
                write_opt_str_prop(out, "label", edge.label.as_deref());
                if edge.style != EdgeStyle::Solid {
                    let _ = write!(out, " style={}", fmt_str(edge.style.as_str()));
                }
                if edge.active {
                    out.push_str(" active=#true");
                }
                write_opt_str_prop(out, "tooltip", edge.tooltip.as_deref());
                out.push('\n');
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Chart {
            id,
            title,
            caption,
            height,
            chart_type,
            x_labels,
            x_unit,
            y_unit,
            value_labels,
            scale,
            series,
        } => {
            push_indent(out, depth);
            let _ = write!(
                out,
                "chart id={} type={}",
                fmt_str(id),
                fmt_str(chart_type.as_str())
            );
            write_opt_str_prop(out, "title", title.as_deref());
            write_opt_str_prop(out, "caption", caption.as_deref());
            write_opt_u16_prop(out, "height", *height);
            if !x_labels.is_empty() {
                let _ = write!(out, " x-labels={}", fmt_str(&x_labels.join(",")));
            }
            write_opt_str_prop(out, "x-unit", x_unit.as_deref());
            write_opt_str_prop(out, "y-unit", y_unit.as_deref());
            if *value_labels {
                out.push_str(" value-labels=#true");
            }
            write_opt_str_prop(out, "scale", scale.as_deref());
            out.push_str(" {\n");
            for s in series {
                push_indent(out, depth + 1);
                let _ = write!(out, "series {}", fmt_str(&s.name));
                write_opt_str_prop(out, "color", s.color.as_deref());
                out.push_str(" {\n");
                for pt in &s.data {
                    write_chart_point(out, pt, depth + 2);
                }
                push_indent(out, depth + 1);
                out.push_str("}\n");
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
        Node::Svg {
            id,
            title,
            caption,
            height,
            source,
            tooltips,
            select_key,
            highlight_key,
        } => {
            push_indent(out, depth);
            let _ = write!(out, "svg id={}", fmt_str(id));
            write_opt_str_prop(out, "title", title.as_deref());
            write_opt_str_prop(out, "caption", caption.as_deref());
            write_opt_u16_prop(out, "height", *height);
            write_opt_str_prop(out, "select-key", select_key.as_deref());
            write_opt_str_prop(out, "highlight-key", highlight_key.as_deref());
            out.push_str(" {\n");
            push_indent(out, depth + 1);
            out.push_str("source ");
            write_string_arg(out, source, depth + 1);
            out.push('\n');
            for (k, v) in tooltips {
                push_indent(out, depth + 1);
                let _ = writeln!(out, "tooltip {} {}", fmt_str(k), fmt_str(v));
            }
            push_indent(out, depth);
            out.push_str("}\n");
        }
    }
}

fn write_log_entry(out: &mut String, entry: &LogEntry, depth: usize) {
    push_indent(out, depth);
    out.push_str("entry");
    write_opt_str_prop(out, "ts", entry.ts.as_deref());
    let _ = write!(out, " level={}", fmt_str(entry.level.as_str()));
    write_opt_str_prop(out, "source", entry.source.as_deref());
    let _ = writeln!(out, " {}", fmt_str(&entry.message));
}

fn write_chart_point(out: &mut String, pt: &ChartPoint, depth: usize) {
    push_indent(out, depth);
    match pt.x {
        Some(x) => {
            let _ = writeln!(out, "point x={} y={}", fmt_f64(x), fmt_f64(pt.y));
        }
        None => {
            let _ = writeln!(out, "point {}", fmt_f64(pt.y));
        }
    }
}

fn write_state_props(out: &mut String, state: &StateMap) {
    for (k, v) in state {
        let _ = write!(out, " {}=", fmt_ident(k));
        match v {
            StateValue::Str(s) => out.push_str(&fmt_str(s)),
            StateValue::Int(i) => {
                let _ = write!(out, "{i}");
            }
            StateValue::Float(f) => out.push_str(&fmt_f64(*f)),
            StateValue::Bool(b) => {
                let _ = write!(out, "#{b}");
            }
            StateValue::List(items) => out.push_str(&fmt_str(&items.join(","))),
            StateValue::Range { start, end } => {
                out.push_str(&fmt_str(&format!("{start}..{end}")));
            }
        }
    }
}

fn write_opt_str_prop(out: &mut String, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        let _ = write!(out, " {key}={}", fmt_str(v));
    }
}

fn write_opt_u16_prop(out: &mut String, key: &str, value: Option<u16>) {
    if let Some(v) = value {
        let _ = write!(out, " {key}={v}");
    }
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("    ");
    }
}

fn fmt_ident(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !s.chars().next().unwrap().is_ascii_digit()
    {
        s.to_string()
    } else {
        fmt_str(s)
    }
}

fn fmt_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_string_arg(out: &mut String, s: &str, depth: usize) {
    if !s.contains('\n') || !can_emit_raw_multiline(s) {
        out.push_str(&fmt_str(s));
        return;
    }

    let mut hash_count = 1usize;
    loop {
        let closing = format!("\"\"\"{}", "#".repeat(hash_count));
        if !s.contains(&closing) {
            break;
        }
        hash_count += 1;
    }

    let hashes = "#".repeat(hash_count);
    let indent = "    ".repeat(depth);
    let _ = writeln!(out, "{hashes}\"\"\"");
    for line in s.split('\n') {
        if !line.is_empty() {
            out.push_str(&indent);
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str(&indent);
    out.push_str("\"\"\"");
    out.push_str(&hashes);
}

fn can_emit_raw_multiline(s: &str) -> bool {
    // KDL v2 normalizes multiline newlines to `\n` and forbids control chars other than `\n`/`\t`.
    !s.chars()
        .any(|c| c == '\r' || (c.is_control() && c != '\n' && c != '\t'))
}

fn fmt_f64(v: f64) -> String {
    if v.is_nan() {
        "#nan".into()
    } else if v.is_infinite() {
        if v.is_sign_positive() {
            "#inf".into()
        } else {
            "#-inf".into()
        }
    } else {
        let s = format!("{v}");
        if s.contains('.') || s.contains('e') || s.contains('E') {
            s
        } else {
            format!("{s}.0")
        }
    }
}

fn fmt_ranges(ranges: &[(usize, usize)]) -> String {
    ranges
        .iter()
        .map(|(a, b)| format!("{a}..{b}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_sample_document() -> Document {
        let mut root_state = StateMap::new();
        root_state.insert("selected".into(), StateValue::Str("gateway".into()));
        root_state.insert(
            "highlight".into(),
            StateValue::List(vec!["gateway".into(), "auth".into()]),
        );
        root_state.insert(
            "code-focus".into(),
            StateValue::Range { start: 14, end: 32 },
        );
        root_state.insert("debug".into(), StateValue::Bool(true));
        root_state.insert("retries".into(), StateValue::Int(3));

        let mut step_state = StateMap::new();
        step_state.insert("selected".into(), StateValue::Str("auth".into()));
        step_state.insert(
            "highlight".into(),
            StateValue::List(vec!["auth".into(), "db".into()]),
        );

        let mut md_by_sel = BTreeMap::new();
        md_by_sel.insert("gateway".into(), "Gateway handles **TLS**\nline 2".into());

        let mut code_by_sel = BTreeMap::new();
        code_by_sel.insert(
            "gateway".into(),
            CodeVariant {
                lang: Some("go".into()),
                path: None,
                lines: None,
                source: "func serve() {\n\tfmt.Println(\"ok\")\n}".into(),
                start_line: 10,
                highlight_lines: vec![(10, 12)],
            },
        );

        let mut svg_tooltips = BTreeMap::new();
        svg_tooltips.insert("box1".into(), "Ingress box".into());

        Document {
            v: 1,
            title: "Architecture Walkthrough".into(),
            subtitle: Some("Production request flow".into()),
            width: Some(0.74),
            height: Some(0.78),
            state: root_state,
            steps: vec![Step {
                id: Some("s1".into()),
                title: "Step 1".into(),
                body: "First line\nSecond line with \"quotes\"".into(),
                state: step_state,
            }],
            root: Node::Split {
                direction: SplitDirection::Horizontal,
                ratio: Some(0.6),
                weights: vec![3, 2],
                children: vec![
                    Node::Tabs {
                        id: "main-tabs".into(),
                        items: vec![
                            TabItem {
                                id: Some("t-graph".into()),
                                title: "Topology".into(),
                                node: Node::Graph {
                                    id: "arch".into(),
                                    title: Some("Services".into()),
                                    caption: Some("Primary request graph".into()),
                                    height: Some(14),
                                    direction: GraphDirection::Lr,
                                    nodes: vec![
                                        GraphNode {
                                            id: "gateway".into(),
                                            label: "Gateway".into(),
                                            sublabel: Some("envoy".into()),
                                            status: Some(Status::Ok),
                                            group: Some("edge".into()),
                                            shape: NodeShape::Rounded,
                                            tooltip: Some("Edge proxy".into()),
                                        },
                                        GraphNode {
                                            id: "auth".into(),
                                            label: "Auth".into(),
                                            sublabel: None,
                                            status: Some(Status::Active),
                                            group: Some("core".into()),
                                            shape: NodeShape::Diamond,
                                            tooltip: None,
                                        },
                                    ],
                                    edges: vec![GraphEdge {
                                        from: "gateway".into(),
                                        to: "auth".into(),
                                        label: Some("gRPC".into()),
                                        style: EdgeStyle::Dashed,
                                        active: true,
                                        tooltip: Some("mTLS channel".into()),
                                    }],
                                    highlight_key: Some("highlight".into()),
                                    select_key: Some("selected".into()),
                                },
                            },
                            TabItem {
                                id: Some("t-chart".into()),
                                title: "Latency".into(),
                                node: Node::Chart {
                                    id: "lat".into(),
                                    title: Some("P99 Latency".into()),
                                    caption: None,
                                    height: None,
                                    chart_type: ChartType::Line,
                                    x_labels: vec!["t0".into(), "t1".into(), "t2".into()],
                                    x_unit: None,
                                    y_unit: Some("ms".into()),
                                    value_labels: false,
                                    scale: Some("lat".into()),
                                    series: vec![ChartSeries {
                                        name: "p99".into(),
                                        color: Some("#60A5FA".into()),
                                        data: vec![
                                            ChartPoint { x: None, y: 12.5 },
                                            ChartPoint {
                                                x: Some(1.0),
                                                y: 18.0,
                                            },
                                        ],
                                    }],
                                },
                            },
                            TabItem {
                                id: Some("t-svg".into()),
                                title: "Custom".into(),
                                node: Node::Svg {
                                    id: "custom-svg".into(),
                                    title: Some("Diagram".into()),
                                    caption: None,
                                    height: None,
                                    source: "<svg viewBox=\"0 0 100 100\">\n  <rect id=\"box1\" fill=\"var(--surface)\"/>\n</svg>".into(),
                                    tooltips: svg_tooltips,
                                    select_key: Some("selected".into()),
                                    highlight_key: Some("highlight".into()),
                                },
                            },
                        ],
                    },
                    Node::Split {
                        direction: SplitDirection::Vertical,
                        ratio: None,
                        weights: vec![1, 1, 1, 1, 1],
                        children: vec![
                            Node::Markdown {
                                id: Some("notes".into()),
                                title: Some("Notes".into()),
                                tone: None,
                                height: None,
                                content: "# Overview\n\nAll systems green.".into(),
                                by_selected: md_by_sel,
                            },
                            Node::Code {
                                id: Some("src".into()),
                                title: Some("Handler".into()),
                                lang: Some("rust".into()),
                                path: None,
                                lines: None,
                                height: None,
                                source: "fn main() {\n    println!(\"hi\");\n}".into(),
                                start_line: 1,
                                highlight_lines: vec![(1, 3)],
                                focus_key: Some("code-focus".into()),
                                by_selected: code_by_sel,
                            },
                            Node::Stat {
                                id: Some("stats".into()),
                                title: Some("KPIs".into()),
                                height: None,
                                items: vec![StatItem {
                                    id: "rps".into(),
                                    label: "RPS".into(),
                                    value: "1.4k".into(),
                                    delta: Some("+12%".into()),
                                    status: Some(Status::Ok),
                                    sparkline: vec![10.0, 14.5, 22.0],
                                }],
                            },
                            Node::Table {
                                id: Some("tbl".into()),
                                title: Some("Instances".into()),
                                height: None,
                                columns: vec!["Host".into(), "State".into()],
                                rows: vec![TableRow {
                                    id: Some("r1".into()),
                                    status: Some(Status::Warn),
                                    cells: vec!["gw-1".into(), "draining".into()],
                                }],
                            },
                            Node::Log {
                                id: "logs".into(),
                                title: Some("Stream".into()),
                                height: None,
                                entries: vec![LogEntry {
                                    ts: Some("12:00:01".into()),
                                    level: LogLevel::Warn,
                                    source: Some("gateway".into()),
                                    message: "high retry rate".into(),
                                }],
                            },
                        ],
                    },
                ],
            },
        }
    }

    #[test]
    fn kdl_v2_round_trips_all_ten_primitives() {
        let doc = full_sample_document();
        let formatted = format_document_kdl(&doc);
        assert!(formatted.contains("#\"\"\""));
        assert!(formatted.contains("#true"));

        let parsed = parse_document(&formatted, "roundtrip.kdl").expect("parse formatted KDL");
        assert_eq!(parsed, doc);
    }

    #[test]
    fn parses_kdl_v1_booleans_via_fallback() {
        let v1 = r#"
            viz v=1 title="Legacy" {
                graph id="g" {
                    node "a" label="A"
                    node "b" label="B"
                    edge "a" "b" active=true
                }
            }
        "#;
        let doc = parse_document(v1, "legacy.kdl").expect("v1 fallback parse");
        match doc.root {
            Node::Graph { edges, .. } => assert!(edges[0].active),
            other => panic!("unexpected root: {other:?}"),
        }
    }

    #[test]
    fn parses_json_document_fallback() {
        let doc = full_sample_document();
        let json = serde_json::to_string_pretty(&doc).unwrap();
        let parsed = parse_document(&json, "doc.json").expect("parse JSON document");
        assert_eq!(parsed, doc);
    }

    #[test]
    fn formats_and_parses_all_event_variants() {
        let mut st = StateMap::new();
        st.insert("selected".into(), StateValue::Str("auth".into()));
        let events = vec![
            Event::SetState(st),
            Event::SetNode {
                graph_id: "arch".into(),
                node_id: "gateway".into(),
                label: Some("GW".into()),
                sublabel: Some("v2".into()),
                status: Some(Status::Warn),
                tooltip: Some("Updated".into()),
            },
            Event::SetEdge {
                graph_id: "arch".into(),
                from: "gateway".into(),
                to: "auth".into(),
                label: Some("HTTP/2".into()),
                active: Some(false),
                tooltip: None,
            },
            Event::SetStat {
                stat_id: "stats".into(),
                item_id: "rps".into(),
                value: Some("2.0k".into()),
                delta: Some("+40%".into()),
                status: Some(Status::Ok),
                sparkline_push: Some(33.5),
            },
            Event::LogAppend {
                target: "logs".into(),
                entries: vec![LogEntry {
                    ts: Some("12:00:02".into()),
                    level: LogLevel::Error,
                    source: Some("auth".into()),
                    message: "token expired".into(),
                }],
            },
            Event::ChartAppend {
                target: "lat".into(),
                series: "p99".into(),
                x_label: Some("t3".into()),
                points: vec![ChartPoint { x: None, y: 24.5 }],
                max_points: Some(50),
            },
            Event::Replace {
                target: "notes".into(),
                node: Box::new(Node::Markdown {
                    id: Some("notes".into()),
                    title: Some("Replaced".into()),
                    tone: None,
                    height: None,
                    content: "New body".into(),
                    by_selected: BTreeMap::new(),
                }),
            },
            Event::Close,
        ];

        let mut chunk = String::new();
        for ev in &events {
            chunk.push_str(&format_event_kdl(ev));
        }
        let parsed = parse_events(&chunk, "events.kdl").expect("parse formatted events");
        assert_eq!(parsed, events);
    }

    #[test]
    fn parses_chart_append_positional_numeric_args() {
        let kdl = r#"
            chart-append "latency" series="p99" x-label="12:05" max-points=12 26.1
            chart-append "latency" "p99" 26.1 28.4
        "#;
        let parsed = parse_events(kdl, "chart_append.kdl").expect("parse positional chart-append");
        assert_eq!(
            parsed,
            vec![
                Event::ChartAppend {
                    target: "latency".into(),
                    series: "p99".into(),
                    x_label: Some("12:05".into()),
                    points: vec![ChartPoint { x: None, y: 26.1 }],
                    max_points: Some(12),
                },
                Event::ChartAppend {
                    target: "latency".into(),
                    series: "p99".into(),
                    x_label: None,
                    points: vec![
                        ChartPoint { x: None, y: 26.1 },
                        ChartPoint { x: None, y: 28.4 },
                    ],
                    max_points: None,
                },
            ]
        );
    }

    #[test]
    fn parses_ergonomic_kdl_shorthands() {
        let kdl = r##"
            viz v=1 title="Shorthand" {
                state selected="gateway" highlight="gateway,auth" code-focus="14..32"
                step "Intro" selected="a" highlight="a,b" code-focus="1..10" {
                    markdown """
                    Step body
                    """
                }
                split direction="horizontal" weights="2,1" {
                    markdown id="detail" title="Notes" """
                    Inline markdown
                    """
                    chart id="c" type="bar" x-labels="q1,q2" {
                        series "rev" data="10,20"
                    }
                }
            }
        "##;
        let doc = parse_document(kdl, "shorthand.kdl").expect("parse ergonomic kdl");
        assert_eq!(
            doc.state.get("highlight"),
            Some(&StateValue::List(vec!["gateway".into(), "auth".into()]))
        );
        assert_eq!(
            doc.state.get("code-focus"),
            Some(&StateValue::Range { start: 14, end: 32 })
        );
        assert_eq!(doc.steps.len(), 1);
        assert_eq!(doc.steps[0].body, "Step body");
        assert_eq!(
            doc.steps[0].state.get("code-focus"),
            Some(&StateValue::Range { start: 1, end: 10 })
        );
    }

    #[test]
    fn round_trips_scroll_hbar_tone_caption_and_height() {
        let kdl = r##"
            viz v=1 title="Benchmark Report" {
                scroll id="report" {
                    markdown id="finding" title="Key Finding" tone="warn" height=5 """
                    Warm cache builds are **3.4x faster**.
                    """
                    chart id="build" title="Build Time" type="horizontal-bar" x-labels="Cold,Warm" x-unit="seconds" value-labels=#true caption="Lower is better (n=5 median)" height=14 {
                        series "main" data="42.1,12.4"
                        series "branch" data="38.5,11.2"
                    }
                }
            }
        "##;
        let parsed = parse_document(kdl, "report.kdl").expect("parse report kdl");
        let formatted = format_document_kdl(&parsed);
        assert!(formatted.contains("scroll id=\"report\""));
        assert!(formatted.contains("tone=\"warn\""));
        assert!(formatted.contains("type=\"hbar\""));
        assert!(formatted.contains("value-labels=#true"));
        assert!(formatted.contains("x-unit=\"seconds\""));
        assert!(formatted.contains("caption=\"Lower is better (n=5 median)\""));
        assert!(formatted.contains("height=14"));

        let reparsed = parse_document(&formatted, "report_rt.kdl").expect("reparse report kdl");
        assert_eq!(reparsed, parsed);

        let code_ptr = r#"
            viz v=1 title="Code Pointer" {
                code id="src" title="theme.rs" path="src/theme.rs" lines="1..30"
            }
        "#;
        let parsed_ptr = parse_document(code_ptr, "ptr.kdl").expect("parse code path");
        let fmt_ptr = format_document_kdl(&parsed_ptr);
        assert!(fmt_ptr.contains("path=\"src/theme.rs\""));
        assert!(fmt_ptr.contains("lines=\"1..30\""));
        assert_eq!(parse_document(&fmt_ptr, "ptr_rt.kdl").unwrap(), parsed_ptr);
    }

    #[test]
    fn reports_miette_diagnostics_on_invalid_input() {
        let bad_syntax = "viz v=1 title=\"unterminated";
        let err = parse_document(bad_syntax, "bad.kdl").unwrap_err();
        assert!(format!("{err:?}").contains("bad.kdl"));

        let bad_version = "viz v=2 title=\"Future\" { markdown \"hi\" }";
        let err = parse_document(bad_version, "v2.kdl").unwrap_err();
        assert!(err.to_string().contains("unsupported document version"));

        let bad_node = "viz v=1 title=\"Bad\" { mystery id=\"x\" }";
        let err = parse_document(bad_node, "bad_node.kdl").unwrap_err();
        assert!(err.to_string().contains("unknown node kind"));

        let bad_enum = "viz v=1 title=\"Bad\" { graph id=\"g\" direction=\"diagonal\" }";
        let err = parse_document(bad_enum, "bad_enum.kdl").unwrap_err();
        assert!(err.to_string().contains("invalid graph direction"));

        let bad_width = "viz v=1 title=\"Bad\" width=1.5 { markdown \"hi\" }";
        let err = parse_document(bad_width, "bad_width.kdl").unwrap_err();
        assert!(err.to_string().contains("out of range"));

        let bad_height = "viz v=1 title=\"Bad\" height=0.05 { markdown \"hi\" }";
        let err = parse_document(bad_height, "bad_height.kdl").unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }
}
