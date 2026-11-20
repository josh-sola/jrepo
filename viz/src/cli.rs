use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand, ValueEnum};
use miette::{Context, IntoDiagnostic, miette};
use serde_json::Value;

use crate::kdl_codec::{format_document_kdl, format_node_kdl, parse_document, parse_events};
use crate::model::{
    ChartPoint, ChartSeries, ChartType, Document, Event, LogEntry, LogLevel, Node, StateMap,
    StateValue, Status,
};
use crate::render::raster::rasterize_svg;
use crate::render::{FollowTarget, run_tui};
use crate::rex::{
    CommandRunner, FloatingLayerRequest, SplitDir, SplitRequest, SystemCommandRunner, close_layer,
    open_floating_layer, open_split,
};
use crate::session::{
    RexMeta, append_events, create_session, default_base_dir, events_path, list_sessions,
    load_meta, load_store, load_store_with_offset, update_meta,
};
use crate::share::{
    DEFAULT_PLANHUB_URL, ShareRequest, post_share, request_body, share_from_file,
    share_from_session,
};
use crate::store::{Store, compute_chart_scales, resolve_code_file};
use crate::svg::chart::{ChartRenderInput, render_chart_svg};
use crate::svg::graph::{GraphRenderInput, render_graph_svg};
use crate::svg::hit_test::prepare_custom_svg;
use crate::svg::selection;
use crate::theme::{Theme, ThemeMode};
use crate::upgrade::{VERSION, run_upgrade};

const GUIDE_TEXT: &str = include_str!("../README.md");

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliTheme {
    Auto,
    Light,
    Dark,
}

impl From<CliTheme> for ThemeMode {
    fn from(value: CliTheme) -> Self {
        match value {
            CliTheme::Auto => Self::Auto,
            CliTheme::Light => Self::Light,
            CliTheme::Dark => Self::Dark,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliSplitDir {
    Right,
    Left,
    Above,
    Below,
}

impl From<CliSplitDir> for SplitDir {
    fn from(value: CliSplitDir) -> Self {
        match value {
            CliSplitDir::Right => Self::Right,
            CliSplitDir::Left => Self::Left,
            CliSplitDir::Above => Self::Above,
            CliSplitDir::Below => Self::Below,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliChartType {
    Hbar,
    Bar,
    Line,
    Area,
    Scatter,
}

impl From<CliChartType> for ChartType {
    fn from(value: CliChartType) -> Self {
        match value {
            CliChartType::Hbar => Self::HBar,
            CliChartType::Bar => Self::Bar,
            CliChartType::Line => Self::Line,
            CliChartType::Area => Self::Area,
            CliChartType::Scatter => Self::Scatter,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "viz",
    version = VERSION,
    about = "Declarative terminal-native visualization engine",
    after_help = "Run `viz guide` for the complete KDL v2 schema, SVG token reference, and live streaming guide."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a session from a KDL or JSON document and open it in Rex
    Open {
        /// Path to a .viz.kdl or .json file (reads stdin if omitted or `-`)
        file: Option<PathBuf>,
        /// Floating layer bounds as x,y,w,h fractions (0..1)
        #[arg(long)]
        bounds: Option<String>,
        /// Open in a Rex split instead of a floating layer
        #[arg(long, value_enum)]
        split: Option<CliSplitDir>,
        /// Target Rex window ID
        #[arg(long)]
        window: Option<String>,
        /// Override document title
        #[arg(long)]
        title: Option<String>,
        /// Color theme
        #[arg(long, value_enum, default_value_t = CliTheme::Auto)]
        theme: CliTheme,
        /// Focus the new Rex layer or split
        #[arg(long)]
        focus: bool,
        /// Create the session on disk without spawning a Rex viewer
        #[arg(long)]
        no_rex: bool,
    },
    /// Render a static file or follow a live session in the current terminal
    View {
        /// Path to a .viz.kdl or .json file (reads stdin if omitted and --follow is unset)
        file: Option<PathBuf>,
        /// Follow a live session by ID
        #[arg(long)]
        follow: Option<String>,
        /// Color theme
        #[arg(long, value_enum, default_value_t = CliTheme::Auto)]
        theme: CliTheme,
    },
    /// Append live updates to an existing session
    Patch {
        /// Session ID
        id: String,
        /// Read KDL/JSON event ops from file instead of stdin
        #[arg(long)]
        file: Option<PathBuf>,
        /// Advance walkthrough to step index (0-based)
        #[arg(long)]
        step: Option<usize>,
        /// Set state key=val (repeatable)
        #[arg(long = "set-state")]
        set_state: Vec<String>,
        /// Update graph node status as GRAPH:NODE=STATUS (repeatable)
        #[arg(long = "node-status")]
        node_status: Vec<String>,
        /// Update stat item value as STAT:ITEM=VALUE (repeatable)
        #[arg(long = "stat")]
        stat: Vec<String>,
    },
    /// Append log entries to a log pane in a live session
    Log {
        /// Session ID
        id: String,
        /// Target log pane ID
        pane_id: String,
        /// Log level (trace, debug, info, warn, error)
        #[arg(long, default_value = "info")]
        level: String,
        /// Optional log source label
        #[arg(long)]
        source: Option<String>,
        /// Message words (reads stdin lines if omitted)
        message: Vec<String>,
    },
    /// Export the folded current state of a session as KDL or JSON
    Export {
        /// Session ID
        id: String,
        /// Output file path (writes stdout if omitted)
        out: Option<PathBuf>,
        /// Output JSON instead of KDL v2
        #[arg(long)]
        json: bool,
    },
    /// List active and recent sessions
    Ls {
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// Mark a session closed and close its Rex floating layer if open
    Close {
        /// Session ID
        id: String,
    },
    /// Validate a KDL or JSON visualization document and run quality checks
    Check {
        /// File to validate (reads stdin if `-`)
        file: PathBuf,
    },
    /// Import CSV, TSV, or JSON array data into a formatted KDL v2 chart node
    ChartFrom {
        /// Input file path (reads stdin if omitted or `-`)
        file: Option<PathBuf>,
        /// Chart type
        #[arg(long = "type", value_enum, default_value_t = CliChartType::Hbar)]
        chart_type: CliChartType,
        /// Chart node ID
        #[arg(long, default_value = "chart")]
        id: String,
        /// Chart title
        #[arg(long)]
        title: Option<String>,
        /// Category / X column name (defaults to first column)
        #[arg(long = "x-col")]
        x_col: Option<String>,
        /// Comma-separated numeric series column names (defaults to all numeric columns)
        #[arg(long = "y-cols")]
        y_cols: Option<String>,
        /// Axis unit label
        #[arg(long)]
        unit: Option<String>,
        /// Shared named scale group
        #[arg(long)]
        scale: Option<String>,
        /// Display numeric value labels on bars
        #[arg(long = "value-labels")]
        value_labels: bool,
    },
    /// Rasterize a graphic pane directly to a PNG image for self-inspection
    Render {
        /// Input .viz.kdl or .json file (reads stdin if `-`)
        file: PathBuf,
        /// Output PNG path
        #[arg(long)]
        png: PathBuf,
        /// Output image width in pixels
        #[arg(long, default_value_t = 1200)]
        width: u32,
        /// Output image height in pixels
        #[arg(long, default_value_t = 720)]
        height: u32,
        /// Color theme
        #[arg(long, value_enum, default_value_t = CliTheme::Light)]
        theme: CliTheme,
        /// Graphic pane ID (defaults to first graph/chart/svg)
        #[arg(long)]
        pane: Option<String>,
        /// Walkthrough step index (0-based) to apply before rendering
        #[arg(long)]
        step: Option<usize>,
    },
    /// Upload a document or session (with its history) to planhub and print the link
    Share {
        /// A .viz.kdl or .json file, or a session ID
        target: String,
        /// Override the plan title (defaults to the document title)
        #[arg(long)]
        title: Option<String>,
        /// Hide the plan from planhub's public list
        #[arg(long)]
        private: bool,
        /// Share a session's current state without its event history
        #[arg(long)]
        no_history: bool,
        /// Print planhub's full JSON response instead of the URL
        #[arg(long)]
        json: bool,
    },
    /// Upgrade viz to the latest published GitHub release
    Upgrade {
        /// Only report the installed and latest versions
        #[arg(long)]
        check: bool,
    },
    /// Print the embedded authoring guide and KDL v2 reference
    Guide,
}

struct OpenArgs {
    file: Option<PathBuf>,
    bounds: Option<String>,
    split: Option<CliSplitDir>,
    window: Option<String>,
    title: Option<String>,
    theme: ThemeMode,
    focus: bool,
    no_rex: bool,
}

struct ChartFromArgs {
    file: Option<PathBuf>,
    chart_type: ChartType,
    id: String,
    title: Option<String>,
    x_col: Option<String>,
    y_cols: Option<String>,
    unit: Option<String>,
    scale: Option<String>,
    value_labels: bool,
}

struct RenderArgs {
    file: PathBuf,
    png: PathBuf,
    width: u32,
    height: u32,
    theme: ThemeMode,
    pane: Option<String>,
    step: Option<usize>,
}

pub fn run() -> miette::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Open {
            file,
            bounds,
            split,
            window,
            title,
            theme,
            focus,
            no_rex,
        } => cmd_open(OpenArgs {
            file,
            bounds,
            split,
            window,
            title,
            theme: theme.into(),
            focus,
            no_rex,
        }),
        Command::View {
            file,
            follow,
            theme,
        } => cmd_view(file, follow, theme.into()),
        Command::Patch {
            id,
            file,
            step,
            set_state,
            node_status,
            stat,
        } => cmd_patch(&id, file, step, &set_state, &node_status, &stat),
        Command::Log {
            id,
            pane_id,
            level,
            source,
            message,
        } => cmd_log(&id, &pane_id, &level, source, &message),
        Command::Export { id, out, json } => cmd_export(&id, out, json),
        Command::Ls { json } => cmd_ls(json),
        Command::Close { id } => cmd_close(&id),
        Command::Check { file } => cmd_check(&file),
        Command::ChartFrom {
            file,
            chart_type,
            id,
            title,
            x_col,
            y_cols,
            unit,
            scale,
            value_labels,
        } => cmd_chart_from(ChartFromArgs {
            file,
            chart_type: chart_type.into(),
            id,
            title,
            x_col,
            y_cols,
            unit,
            scale,
            value_labels,
        }),
        Command::Render {
            file,
            png,
            width,
            height,
            theme,
            pane,
            step,
        } => cmd_render(RenderArgs {
            file,
            png,
            width,
            height,
            theme: theme.into(),
            pane,
            step,
        }),
        Command::Share {
            target,
            title,
            private,
            no_history,
            json,
        } => cmd_share(&target, title.as_deref(), private, no_history, json),
        Command::Upgrade { check } => {
            let env: HashMap<String, String> = std::env::vars().collect();
            let runner = SystemCommandRunner;
            let msg = run_upgrade(&runner, VERSION, check, &env)?;
            println!("{msg}");
            Ok(())
        }
        Command::Guide => {
            print!("{GUIDE_TEXT}");
            Ok(())
        }
    }
}

fn cmd_open(args: OpenArgs) -> miette::Result<()> {
    let source_dir = resolve_input_base_dir(args.file.as_deref());
    let (src, name) = read_input(args.file.as_deref())?;
    let mut doc = parse_document(&src, &name)?;
    if let Some(override_title) = args.title {
        doc.title = override_title;
    }
    let bounds = resolve_open_bounds(args.bounds.as_deref(), doc.width, doc.height)?;
    let doc_title = doc.title.clone();
    let base_dir = default_base_dir()?;
    let mut meta = create_session(
        &base_dir,
        doc,
        RexMeta {
            source_dir: source_dir.map(|p| p.to_string_lossy().into_owned()),
            ..RexMeta::default()
        },
    )?;

    if !args.no_rex
        && let Ok(rex_session) = std::env::var("REX_SESSION")
        && !rex_session.trim().is_empty()
    {
        let exe = std::env::current_exe()
            .into_diagnostic()?
            .to_string_lossy()
            .into_owned();
        let runner = SystemCommandRunner;
        let res = match args.split.map(Into::into) {
            Some(direction) => open_split(
                &runner,
                &SplitRequest {
                    rex_session: rex_session.clone(),
                    viz_id: meta.id.clone(),
                    exe,
                    direction,
                    title: doc_title,
                    focus: args.focus,
                    theme: args.theme,
                    window_id: args.window,
                },
            ),
            None => open_floating_layer(
                &runner,
                &FloatingLayerRequest {
                    rex_session: rex_session.clone(),
                    viz_id: meta.id.clone(),
                    exe,
                    bounds,
                    focus: args.focus,
                    theme: args.theme,
                    window_id: args.window,
                    title: Some(doc_title),
                },
            ),
        };
        match res {
            Ok(layer) => {
                meta.rex_session = Some(rex_session);
                meta.rex_layer_id = layer.layer_id;
                meta.rex_block_id = layer.block_id;
                update_meta(&base_dir, &meta)?;
            }
            Err(err) => {
                eprintln!("viz: warning: could not open Rex viewer: {err}");
            }
        }
    }

    println!("{}", meta.id);
    Ok(())
}

fn cmd_view(file: Option<PathBuf>, follow: Option<String>, theme: ThemeMode) -> miette::Result<()> {
    if let Some(id) = follow {
        let base_dir = default_base_dir()?;
        let (store, byte_offset) = load_store_with_offset(&base_dir, &id)?;
        let path = events_path(&base_dir, &id);
        return run_tui(
            store,
            theme,
            Some(FollowTarget {
                events_path: path,
                byte_offset,
            }),
        );
    }

    let source_dir = resolve_input_base_dir(file.as_deref());
    let (src, name) = read_input(file.as_deref())?;
    let doc = parse_document(&src, &name)?;
    run_tui(Store::with_base_dir(doc, source_dir), theme, None)
}

fn cmd_patch(
    id: &str,
    file: Option<PathBuf>,
    step: Option<usize>,
    set_state: &[String],
    node_status: &[String],
    stat: &[String],
) -> miette::Result<()> {
    let base_dir = default_base_dir()?;
    let mut events = Vec::new();

    if let Some(path) = file {
        let (src, name) = read_input(Some(&path))?;
        events.extend(parse_events(&src, &name)?);
    }

    let mut state_updates = StateMap::new();
    if let Some(step_idx) = step {
        state_updates.insert("_step".into(), StateValue::Int(step_idx as i64));
    }
    for kv in set_state {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| miette!("expected KEY=VAL for --set-state, got `{kv}`"))?;
        state_updates.insert(
            k.trim().to_string(),
            parse_cli_state_val(k.trim(), v.trim()),
        );
    }
    if !state_updates.is_empty() {
        events.push(Event::SetState(state_updates));
    }

    for spec in node_status {
        let (lhs, status_str) = spec
            .split_once('=')
            .ok_or_else(|| miette!("expected GRAPH:NODE=STATUS for --node-status, got `{spec}`"))?;
        let (graph_id, node_id) = lhs
            .split_once(':')
            .ok_or_else(|| miette!("expected GRAPH:NODE=STATUS for --node-status, got `{spec}`"))?;
        let status = Status::parse(status_str.trim())
            .ok_or_else(|| miette!("invalid status `{status_str}`"))?;
        events.push(Event::SetNode {
            graph_id: graph_id.trim().to_string(),
            node_id: node_id.trim().to_string(),
            label: None,
            sublabel: None,
            status: Some(status),
            tooltip: None,
        });
    }

    for spec in stat {
        let (lhs, val) = spec
            .split_once('=')
            .ok_or_else(|| miette!("expected STAT:ITEM=VALUE for --stat, got `{spec}`"))?;
        let (stat_id, item_id) = lhs
            .split_once(':')
            .ok_or_else(|| miette!("expected STAT:ITEM=VALUE for --stat, got `{spec}`"))?;
        events.push(Event::SetStat {
            stat_id: stat_id.trim().to_string(),
            item_id: item_id.trim().to_string(),
            value: Some(val.trim().to_string()),
            delta: None,
            status: None,
            sparkline_push: None,
        });
    }

    if events.is_empty() {
        let (src, name) = read_input(None)?;
        events.extend(parse_events(&src, &name)?);
    }

    append_events(&base_dir, id, &events)?;
    Ok(())
}

fn cmd_log(
    id: &str,
    pane_id: &str,
    level_raw: &str,
    source: Option<String>,
    message: &[String],
) -> miette::Result<()> {
    let level =
        LogLevel::parse(level_raw).ok_or_else(|| miette!("invalid log level `{level_raw}`"))?;
    let base_dir = default_base_dir()?;
    let ts = Some(current_hms());

    let entries: Vec<LogEntry> = if !message.is_empty() {
        vec![LogEntry {
            ts,
            level,
            source,
            message: message.join(" "),
        }]
    } else {
        let stdin = io::stdin();
        let mut out = Vec::new();
        for line in stdin.lock().lines() {
            let line = line.into_diagnostic()?;
            if line.is_empty() {
                continue;
            }
            out.push(LogEntry {
                ts: Some(current_hms()),
                level,
                source: source.clone(),
                message: line,
            });
        }
        out
    };

    if !entries.is_empty() {
        append_events(
            &base_dir,
            id,
            &[Event::LogAppend {
                target: pane_id.to_string(),
                entries,
            }],
        )?;
    }
    Ok(())
}

fn cmd_export(id: &str, out: Option<PathBuf>, json: bool) -> miette::Result<()> {
    let base_dir = default_base_dir()?;
    let store = load_store(&base_dir, id)?;
    let snapshot = store.export_document();
    let rendered = if json {
        serde_json::to_string_pretty(&snapshot).into_diagnostic()? + "\n"
    } else {
        format_document_kdl(&snapshot)
    };

    match out {
        Some(path) => fs::write(&path, rendered)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to write {}", path.display()))?,
        None => print!("{rendered}"),
    }
    Ok(())
}

fn cmd_share(
    target: &str,
    title: Option<&str>,
    private: bool,
    no_history: bool,
    json: bool,
) -> miette::Result<()> {
    let path = Path::new(target);
    let share = if path.is_file() {
        share_from_file(path)?
    } else {
        share_from_session(&default_base_dir()?, target, no_history)?
    };

    let runner = SystemCommandRunner;
    let host = runner
        .run("hostname", &["-s"])
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty());
    let body = request_body(
        &share,
        &ShareRequest {
            title,
            private,
            host: host.as_deref(),
        },
    )?;
    let planhub = std::env::var("VIZ_PLANHUB_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PLANHUB_URL.to_string());
    let resp = post_share(&runner, &planhub, &body).map_err(|e| miette!("{e}"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp).into_diagnostic()?);
    } else {
        println!("{}", resp["url"].as_str().unwrap_or_default());
    }
    Ok(())
}

fn cmd_ls(json: bool) -> miette::Result<()> {
    let base_dir = default_base_dir()?;
    let sessions = list_sessions(&base_dir)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&sessions).into_diagnostic()?
        );
        return Ok(());
    }
    if sessions.is_empty() {
        println!("No viz sessions.");
        return Ok(());
    }
    for s in sessions {
        let rex = s
            .rex_layer_id
            .as_deref()
            .or(s.rex_block_id.as_deref())
            .unwrap_or("-");
        println!("{:<22} {:<32} {}", s.id, s.title, rex);
    }
    Ok(())
}

fn cmd_close(id: &str) -> miette::Result<()> {
    let base_dir = default_base_dir()?;
    append_events(&base_dir, id, &[Event::Close])?;
    if let Ok(meta) = load_meta(&base_dir, id)
        && let (Some(rex_session), Some(layer_id)) = (meta.rex_session, meta.rex_layer_id)
    {
        let runner = SystemCommandRunner;
        let _ = close_layer(&runner, &rex_session, &layer_id);
    }
    Ok(())
}

fn cmd_check(file: &Path) -> miette::Result<()> {
    let source_dir = resolve_input_base_dir(Some(file));
    let (src, name) = read_input(Some(file))?;
    let doc = parse_document(&src, &name)?;
    let (errors, warnings) = lint_document(&doc, source_dir.as_deref());
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    if !errors.is_empty() {
        for err in &errors {
            eprintln!("error: {err}");
        }
        return Err(miette!(
            "check failed with {} error(s) in {name}",
            errors.len()
        ));
    }
    println!("OK: {} (v{})", doc.title, doc.v);
    Ok(())
}

pub fn lint_document(doc: &Document, base_dir: Option<&Path>) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    lint_node(&doc.root, base_dir, &mut errors, &mut warnings);
    (errors, warnings)
}

fn lint_node(
    node: &Node,
    base_dir: Option<&Path>,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    match node {
        Node::Split { children, .. } | Node::Scroll { children, .. } => {
            for child in children {
                lint_node(child, base_dir, errors, warnings);
            }
        }
        Node::Tabs { items, .. } => {
            for item in items {
                lint_node(&item.node, base_dir, errors, warnings);
            }
        }
        Node::Graph {
            id, nodes, edges, ..
        } => {
            let known: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
            for edge in edges {
                if !known.contains(edge.from.as_str()) {
                    errors.push(format!(
                        "graph `{id}` edge references unknown source node `{}`",
                        edge.from
                    ));
                }
                if !known.contains(edge.to.as_str()) {
                    errors.push(format!(
                        "graph `{id}` edge references unknown target node `{}`",
                        edge.to
                    ));
                }
            }
            if nodes.len() > 25 {
                warnings.push(format!(
                    "graph `{id}` has {} nodes (>25); consider splitting into steps or tabs",
                    nodes.len()
                ));
            }
        }
        Node::Chart {
            id,
            chart_type,
            x_labels,
            series,
            ..
        } => {
            let mut total_bars = 0usize;
            for s in series {
                for (i, pt) in s.data.iter().enumerate() {
                    if !pt.y.is_finite() || pt.x.is_some_and(|x| !x.is_finite()) {
                        errors.push(format!(
                            "chart `{id}` series `{}` point #{i} contains non-finite value",
                            s.name
                        ));
                    }
                }
                if !x_labels.is_empty() && s.data.len() != x_labels.len() {
                    warnings.push(format!(
                        "chart `{id}` series `{}` has {} points but x-labels has {}",
                        s.name,
                        s.data.len(),
                        x_labels.len()
                    ));
                }
                total_bars += s.data.len();
            }
            if matches!(chart_type, ChartType::Bar | ChartType::HBar) && total_bars > 60 {
                warnings.push(format!(
                    "chart `{id}` has {total_bars} bars (>60); dense barcode bars hurt readability"
                ));
            }
        }
        Node::Stat { id, items, .. } => {
            let stat_id = id.as_deref().unwrap_or("stat");
            for item in items {
                if item.sparkline.iter().any(|v| !v.is_finite()) {
                    errors.push(format!(
                        "stat `{stat_id}` item `{}` sparkline contains non-finite value",
                        item.id
                    ));
                }
            }
        }
        Node::Code {
            id,
            path,
            lines,
            by_selected,
            ..
        } => {
            let code_id = id.as_deref().unwrap_or("code");
            if let Some(p) = path.as_deref()
                && let Err(err) = resolve_code_file(p, *lines, base_dir)
            {
                errors.push(format!("code `{code_id}` cannot read `{p}`: {err}"));
            }
            for (case_key, variant) in by_selected {
                if let Some(p) = variant.path.as_deref()
                    && let Err(err) = resolve_code_file(p, variant.lines, base_dir)
                {
                    errors.push(format!(
                        "code `{code_id}` case `{case_key}` cannot read `{p}`: {err}"
                    ));
                }
            }
        }
        Node::Markdown { .. } | Node::Table { .. } | Node::Log { .. } | Node::Svg { .. } => {}
    }
}

fn cmd_chart_from(args: ChartFromArgs) -> miette::Result<()> {
    let (raw, name) = read_input(args.file.as_deref())?;
    let source_label = match args.file.as_deref() {
        Some(p) if p.as_os_str() != "-" => p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&name)
            .to_string(),
        _ => "stdin".to_string(),
    };
    let node = build_chart_from_data(&raw, &source_label, &args)?;
    print!("{}", format_node_kdl(&node));
    Ok(())
}

fn build_chart_from_data(
    raw: &str,
    source_label: &str,
    args: &ChartFromArgs,
) -> miette::Result<Node> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(miette!("input dataset is empty"));
    }

    let (headers, rows) = if trimmed.starts_with('[') {
        parse_json_table(trimmed)?
    } else {
        parse_delimited_table(trimmed)?
    };
    if headers.is_empty() || rows.is_empty() {
        return Err(miette!("dataset has no rows"));
    }

    let x_col_name = args.x_col.clone().unwrap_or_else(|| {
        if args.chart_type == ChartType::Scatter {
            headers[0].clone()
        } else {
            // serde_json::Map sorts keys alphabetically, so pick the first non-numeric
            // column as the category axis before falling back to headers[0].
            headers
                .iter()
                .enumerate()
                .find(|&(idx, _)| {
                    rows.iter()
                        .any(|r| r.get(idx).is_none_or(|v| parse_finite_f64(v).is_err()))
                })
                .map(|(_, h)| h.clone())
                .unwrap_or_else(|| headers[0].clone())
        }
    });
    let x_idx = headers
        .iter()
        .position(|h| h == &x_col_name)
        .ok_or_else(|| miette!("x column `{x_col_name}` not found in headers {headers:?}"))?;

    let y_indices: Vec<usize> = if let Some(ref spec) = args.y_cols {
        spec.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|col| {
                headers
                    .iter()
                    .position(|h| h == col)
                    .ok_or_else(|| miette!("y column `{col}` not found in headers {headers:?}"))
            })
            .collect::<miette::Result<Vec<_>>>()?
    } else {
        headers
            .iter()
            .enumerate()
            .filter(|&(idx, _)| {
                idx != x_idx
                    && rows
                        .iter()
                        .all(|r| r.get(idx).is_some_and(|v| parse_finite_f64(v).is_ok()))
            })
            .map(|(idx, _)| idx)
            .collect()
    };

    if y_indices.is_empty() {
        return Err(miette!(
            "no numeric series columns found (pass --y-cols explicitly)"
        ));
    }

    let mut x_labels = Vec::with_capacity(rows.len());
    let mut series: Vec<ChartSeries> = y_indices
        .iter()
        .map(|&idx| ChartSeries {
            name: headers[idx].clone(),
            color: None,
            data: Vec::with_capacity(rows.len()),
        })
        .collect();

    for (r_idx, row) in rows.iter().enumerate() {
        let x_raw = row.get(x_idx).map(String::as_str).unwrap_or("");
        x_labels.push(x_raw.to_string());
        let scatter_x = if args.chart_type == ChartType::Scatter {
            Some(parse_finite_f64(x_raw).map_err(|_| {
                miette!(
                    "row {}: scatter x value `{x_raw}` is not a finite number",
                    r_idx + 1
                )
            })?)
        } else {
            None
        };

        for (s_pos, &col_idx) in y_indices.iter().enumerate() {
            let cell = row.get(col_idx).map(String::as_str).unwrap_or("");
            let y = parse_finite_f64(cell)
                .map_err(|e| miette!("row {} column `{}`: {e}", r_idx + 1, headers[col_idx]))?;
            series[s_pos].data.push(ChartPoint { x: scatter_x, y });
        }
    }

    let (x_unit, y_unit) = match args.chart_type {
        ChartType::HBar => (args.unit.clone(), None),
        _ => (None, args.unit.clone()),
    };

    Ok(Node::Chart {
        id: args.id.clone(),
        title: args.title.clone(),
        caption: Some(format!("{} rows from {source_label}", rows.len())),
        height: None,
        chart_type: args.chart_type,
        x_labels,
        x_unit,
        y_unit,
        value_labels: args.value_labels,
        scale: args.scale.clone(),
        series,
    })
}

fn parse_finite_f64(raw: &str) -> Result<f64, String> {
    let trimmed = raw.trim();
    let v = trimmed
        .parse::<f64>()
        .map_err(|_| format!("`{trimmed}` is not a number"))?;
    if !v.is_finite() {
        return Err(format!("`{trimmed}` is not finite (NaN/Inf refused)"));
    }
    Ok(v)
}

fn parse_json_table(raw: &str) -> miette::Result<(Vec<String>, Vec<Vec<String>>)> {
    let arr: Vec<Value> = serde_json::from_str(raw)
        .into_diagnostic()
        .wrap_err("expected a JSON array of objects")?;
    if arr.is_empty() {
        return Err(miette!("JSON array is empty"));
    }
    let mut headers: Vec<String> = Vec::new();
    for item in &arr {
        let obj = item
            .as_object()
            .ok_or_else(|| miette!("each JSON array item must be an object"))?;
        for k in obj.keys() {
            if !headers.contains(k) {
                headers.push(k.clone());
            }
        }
    }
    let mut rows = Vec::with_capacity(arr.len());
    for item in &arr {
        let obj = item.as_object().unwrap();
        let row = headers
            .iter()
            .map(|h| match obj.get(h) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::Bool(b)) => b.to_string(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            })
            .collect();
        rows.push(row);
    }
    Ok((headers, rows))
}

fn parse_delimited_table(raw: &str) -> miette::Result<(Vec<String>, Vec<Vec<String>>)> {
    let mut lines = raw.lines().filter(|l| !l.trim().is_empty());
    let header_line = lines
        .next()
        .ok_or_else(|| miette!("CSV/TSV input has no header row"))?;
    let tab_count = header_line.matches('\t').count();
    let comma_count = header_line.matches(',').count();
    let delim = if tab_count > comma_count { '\t' } else { ',' };

    let headers = parse_delimited_row(header_line, delim);
    let mut rows = Vec::new();
    for line in lines {
        rows.push(parse_delimited_row(line, delim));
    }
    Ok((headers, rows))
}

fn parse_delimited_row(line: &str, delim: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
        } else if ch == '"' && current.is_empty() {
            in_quotes = true;
        } else if ch == delim {
            fields.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    fields.push(current.trim().to_string());
    fields
}

fn cmd_render(args: RenderArgs) -> miette::Result<()> {
    let (src, name) = read_input(Some(&args.file))?;
    let doc = parse_document(&src, &name)?;
    let mut store = Store::new(doc);
    if let Some(step) = args.step {
        if step >= store.doc.steps.len() {
            return Err(miette!(
                "step index {step} out of range (document has {} steps)",
                store.doc.steps.len()
            ));
        }
        store.set_step(step);
    }

    let state = store.effective_state();
    let scales = compute_chart_scales(&store.doc.root);
    let target =
        find_graphic_node(&store.doc.root, args.pane.as_deref()).ok_or_else(|| {
            match args.pane.as_deref() {
                Some(id) => miette!("graphic pane `{id}` not found in {name}"),
                None => miette!("no graph, chart, or svg pane found in {name}"),
            }
        })?;

    let theme = match args.theme {
        ThemeMode::Dark => Theme::dark(),
        ThemeMode::Light | ThemeMode::Auto => Theme::light(),
    };

    let (pane_id, bundle) = match target {
        Node::Graph {
            id,
            direction,
            nodes,
            edges,
            highlight_key,
            select_key,
            ..
        } => {
            let (selected, highlighted) =
                selection(&state, select_key.as_deref(), highlight_key.as_deref());
            (
                id.as_str(),
                render_graph_svg(
                    &GraphRenderInput {
                        nodes,
                        edges,
                        direction: *direction,
                        selected,
                        highlighted: &highlighted,
                    },
                    &theme,
                    args.width,
                    args.height,
                ),
            )
        }
        Node::Chart {
            id,
            title,
            chart_type,
            x_labels,
            x_unit,
            y_unit,
            value_labels,
            scale,
            series,
            ..
        } => {
            let max_override = scale.as_deref().and_then(|s| scales.get(s).copied());
            let bundle = render_chart_svg(
                &ChartRenderInput {
                    id,
                    title: title.as_deref(),
                    chart_type: *chart_type,
                    x_labels,
                    x_unit: x_unit.as_deref(),
                    y_unit: y_unit.as_deref(),
                    value_labels: *value_labels,
                    max_override,
                    series,
                },
                &theme,
                args.width,
                args.height,
            )
            .map_err(|e| miette!("{e}"))?;
            (id.as_str(), bundle)
        }
        Node::Svg {
            id,
            source,
            tooltips,
            select_key,
            highlight_key,
            ..
        } => {
            let (selected, highlighted) =
                selection(&state, select_key.as_deref(), highlight_key.as_deref());
            let bundle = prepare_custom_svg(
                source,
                tooltips,
                selected,
                &highlighted,
                &theme,
                args.width,
                args.height,
            )
            .map_err(|e| miette!("{e}"))?;
            (id.as_str(), bundle)
        }
        _ => unreachable!("find_graphic_node only returns graphic variants"),
    };

    let (img, _) =
        rasterize_svg(&bundle.svg, args.width, args.height).map_err(|e| miette!("{e}"))?;
    if let Some(parent) = args.png.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).into_diagnostic()?;
    }
    img.save(&args.png)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to write PNG {}", args.png.display()))?;

    println!(
        "Rendered `{pane_id}` -> {} ({}x{}, {} hit targets)",
        args.png.display(),
        img.width(),
        img.height(),
        bundle.hit_map.targets.len()
    );
    Ok(())
}

fn find_graphic_node<'a>(node: &'a Node, target_id: Option<&str>) -> Option<&'a Node> {
    match node {
        Node::Graph { id, .. } | Node::Chart { id, .. } | Node::Svg { id, .. } => match target_id {
            Some(wanted) if id == wanted => Some(node),
            Some(_) => None,
            None => Some(node),
        },
        Node::Split { children, .. } | Node::Scroll { children, .. } => {
            for child in children {
                if let Some(found) = find_graphic_node(child, target_id) {
                    return Some(found);
                }
            }
            None
        }
        Node::Tabs { items, .. } => {
            for item in items {
                if let Some(found) = find_graphic_node(&item.node, target_id) {
                    return Some(found);
                }
            }
            None
        }
        Node::Markdown { .. }
        | Node::Code { .. }
        | Node::Stat { .. }
        | Node::Table { .. }
        | Node::Log { .. } => None,
    }
}

fn resolve_input_base_dir(path: Option<&Path>) -> Option<PathBuf> {
    match path {
        Some(p) if p.as_os_str() != "-" => {
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::env::current_dir().ok()?.join(p)
            };
            abs.parent().map(Path::to_path_buf)
        }
        _ => std::env::current_dir().ok(),
    }
}

fn read_input(path: Option<&Path>) -> miette::Result<(String, String)> {
    match path {
        Some(p) if p.as_os_str() != "-" => {
            let content = fs::read_to_string(p)
                .into_diagnostic()
                .wrap_err_with(|| format!("failed to read {}", p.display()))?;
            Ok((content, p.display().to_string()))
        }
        _ => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .into_diagnostic()
                .wrap_err("failed to read stdin")?;
            Ok((buf, "<stdin>".into()))
        }
    }
}

pub fn resolve_open_bounds(
    cli_bounds: Option<&str>,
    doc_width: Option<f64>,
    doc_height: Option<f64>,
) -> miette::Result<[f64; 4]> {
    if let Some(raw) = cli_bounds {
        return parse_bounds(raw);
    }
    if doc_width.is_some() || doc_height.is_some() {
        let w = doc_width.unwrap_or(0.68);
        let h = doc_height.unwrap_or(0.76);
        let x = ((1.0 - w) * 0.65).clamp(0.02, (1.0 - w - 0.02).max(0.02));
        let y = ((1.0 - h) * 0.45).clamp(0.02, (1.0 - h - 0.02).max(0.02));
        return Ok([x, y, w, h]);
    }
    Ok([0.18, 0.08, 0.72, 0.78])
}

fn parse_bounds(raw: &str) -> miette::Result<[f64; 4]> {
    let parts: Vec<f64> = raw
        .split(',')
        .map(|s| s.trim().parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| miette!("invalid --bounds `{raw}`; expected 4 comma-separated floats"))?;
    if parts.len() != 4 {
        return Err(miette!(
            "invalid --bounds `{raw}`; expected x,y,w,h (4 floats)"
        ));
    }
    Ok([parts[0], parts[1], parts[2], parts[3]])
}

fn parse_cli_state_val(key: &str, raw: &str) -> StateValue {
    if let Some((lhs, rhs)) = raw.split_once("..")
        && let (Ok(start), Ok(end)) = (lhs.trim().parse::<usize>(), rhs.trim().parse::<usize>())
    {
        return StateValue::Range { start, end };
    }
    if raw == "true" || raw == "#true" {
        return StateValue::Bool(true);
    }
    if raw == "false" || raw == "#false" {
        return StateValue::Bool(false);
    }
    if let Ok(i) = raw.parse::<i64>() {
        return StateValue::Int(i);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return StateValue::Float(f);
    }
    if key == "highlight" || raw.contains(',') {
        return StateValue::List(
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect(),
        );
    }
    StateValue::Str(raw.to_string())
}

fn current_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_open_bounds_follows_cli_doc_default_precedence() {
        let explicit = resolve_open_bounds(Some("0.1,0.2,0.5,0.6"), Some(0.8), Some(0.8)).unwrap();
        assert_eq!(explicit, [0.1, 0.2, 0.5, 0.6]);

        let from_doc = resolve_open_bounds(None, Some(0.56), Some(0.52)).unwrap();
        assert!((from_doc[2] - 0.56).abs() < 1e-6);
        assert!((from_doc[3] - 0.52).abs() < 1e-6);
        assert!((from_doc[0] - (0.44 * 0.65)).abs() < 1e-6);
        assert!((from_doc[1] - (0.48 * 0.45)).abs() < 1e-6);

        let fallback = resolve_open_bounds(None, None, None).unwrap();
        assert_eq!(fallback, [0.18, 0.08, 0.72, 0.78]);
    }

    #[test]
    fn chart_from_parses_csv_tsv_json_and_refuses_nan() {
        let args = ChartFromArgs {
            file: None,
            chart_type: ChartType::HBar,
            id: "bench".into(),
            title: Some("Timings".into()),
            x_col: None,
            y_cols: None,
            unit: Some("seconds".into()),
            scale: Some("sec".into()),
            value_labels: true,
        };

        let csv = "scenario,baseline,cached\ncold,42.1,12.4\nwarm,10.5,2.1\n";
        let node = build_chart_from_data(csv, "bench.csv", &args).unwrap();
        let kdl = format_node_kdl(&node);
        assert!(kdl.contains("type=\"hbar\""));
        assert!(kdl.contains("scale=\"sec\""));
        assert!(kdl.contains("2 rows from bench.csv"));

        let tsv = "scenario\tbaseline\ncold\t42.1\n";
        let tsv_node = build_chart_from_data(tsv, "bench.tsv", &args).unwrap();
        let Node::Chart { series, .. } = tsv_node else {
            panic!("expected chart");
        };
        assert_eq!(series[0].data[0].y, 42.1);

        let json = r#"[{"scenario":"cold","baseline":42.1,"cached":12.4}]"#;
        let json_node = build_chart_from_data(json, "bench.json", &args).unwrap();
        let Node::Chart { series, .. } = json_node else {
            panic!("expected chart");
        };
        assert_eq!(series.len(), 2);

        let nan_csv = "scenario,baseline\ncold,NaN\n";
        assert!(build_chart_from_data(nan_csv, "bad.csv", &args).is_err());
    }

    #[test]
    fn lint_document_catches_unknown_edges_and_barcode_charts() {
        let bad_edge = parse_document(
            r#"viz v=1 title="Bad Graph" {
                graph id="g" {
                    node "a" label="A"
                    edge "a" "missing"
                }
            }"#,
            "bad.kdl",
        )
        .unwrap();
        let (errors, _) = lint_document(&bad_edge, None);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("unknown target node `missing`"))
        );

        let mut pts = Vec::new();
        for i in 0..65 {
            pts.push(format!("{i}"));
        }
        let dense_kdl = format!(
            r#"viz v=1 title="Dense" {{
                chart id="c" type="hbar" x-labels="a,b" {{
                    series "s" data="{}"
                }}
            }}"#,
            pts.join(",")
        );
        let dense = parse_document(&dense_kdl, "dense.kdl").unwrap();
        let (errors, warnings) = lint_document(&dense, None);
        assert!(errors.is_empty());
        assert!(warnings.iter().any(|w| w.contains("barcode")));
        assert!(warnings.iter().any(|w| w.contains("x-labels has 2")));
    }
}
