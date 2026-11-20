use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type StateMap = BTreeMap<String, StateValue>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StateValue {
    Range { start: usize, end: usize },
    Bool(bool),
    Int(i64),
    Float(f64),
    List(Vec<String>),
    Str(String),
}

impl StateValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Vec<String> {
        match self {
            Self::List(items) => items.clone(),
            Self::Str(s) if !s.is_empty() => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    pub fn as_range(&self) -> Option<(usize, usize)> {
        match self {
            Self::Range { start, end } => Some((*start, *end)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub v: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default)]
    pub state: StateMap,
    #[serde(default)]
    pub steps: Vec<Step>,
    pub root: Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub state: StateMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Error,
    Active,
    Idle,
    Muted,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Active => "active",
            Self::Idle => "idle",
            Self::Muted => "muted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ok" => Some(Self::Ok),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            "active" => Some(Self::Active),
            "idle" => Some(Self::Idle),
            "muted" => Some(Self::Muted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

impl SplitDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "horizontal" | "h" => Some(Self::Horizontal),
            "vertical" | "v" => Some(Self::Vertical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphDirection {
    #[default]
    Lr,
    Tb,
}

impl GraphDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lr => "lr",
            Self::Tb => "tb",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "lr" | "LR" => Some(Self::Lr),
            "tb" | "TB" => Some(Self::Tb),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeShape {
    #[default]
    Rounded,
    Box,
    Diamond,
    Pill,
}

impl NodeShape {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rounded => "rounded",
            Self::Box => "box",
            Self::Diamond => "diamond",
            Self::Pill => "pill",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rounded" => Some(Self::Rounded),
            "box" => Some(Self::Box),
            "diamond" => Some(Self::Diamond),
            "pill" => Some(Self::Pill),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeStyle {
    #[default]
    Solid,
    Dashed,
}

impl EdgeStyle {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::Dashed => "dashed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "solid" => Some(Self::Solid),
            "dashed" => Some(Self::Dashed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartType {
    #[default]
    Line,
    Area,
    Bar,
    HBar,
    Scatter,
}

impl ChartType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Area => "area",
            Self::Bar => "bar",
            Self::HBar => "hbar",
            Self::Scatter => "scatter",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "line" => Some(Self::Line),
            "area" => Some(Self::Area),
            "bar" => Some(Self::Bar),
            "hbar" | "horizontal-bar" => Some(Self::HBar),
            "scatter" => Some(Self::Scatter),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeVariant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<(usize, usize)>,
    #[serde(default)]
    pub source: String,
    #[serde(default = "default_start_line")]
    pub start_line: usize,
    #[serde(default)]
    pub highlight_lines: Vec<(usize, usize)>,
}

pub fn infer_lang_from_path(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some("rust"),
        "ts" => Some("ts"),
        "tsx" => Some("tsx"),
        "js" => Some("js"),
        "jsx" => Some("jsx"),
        "py" => Some("python"),
        "go" => Some("go"),
        "sql" => Some("sql"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "sh" => Some("sh"),
        "zsh" => Some("zsh"),
        "md" => Some("md"),
        "kdl" => Some("kdl"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatItem {
    pub id: String,
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default)]
    pub sparkline: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    #[serde(default)]
    pub level: LogLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sublabel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default)]
    pub shape: NodeShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub style: EdgeStyle,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartPoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartSeries {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub data: Vec<ChartPoint>,
}

const fn default_start_line() -> usize {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Node {
    Split {
        direction: SplitDirection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratio: Option<f32>,
        #[serde(default)]
        weights: Vec<u16>,
        children: Vec<Node>,
    },
    Scroll {
        id: String,
        children: Vec<Node>,
    },
    Tabs {
        id: String,
        items: Vec<TabItem>,
    },
    Markdown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tone: Option<Status>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        #[serde(default)]
        content: String,
        #[serde(default)]
        by_selected: BTreeMap<String, String>,
    },
    Code {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<(usize, usize)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        #[serde(default)]
        source: String,
        #[serde(default = "default_start_line")]
        start_line: usize,
        #[serde(default)]
        highlight_lines: Vec<(usize, usize)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus_key: Option<String>,
        #[serde(default)]
        by_selected: BTreeMap<String, CodeVariant>,
    },
    Stat {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        #[serde(default)]
        items: Vec<StatItem>,
    },
    Table {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        #[serde(default)]
        columns: Vec<String>,
        #[serde(default)]
        rows: Vec<TableRow>,
    },
    Log {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        #[serde(default)]
        entries: Vec<LogEntry>,
    },
    Graph {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        #[serde(default)]
        direction: GraphDirection,
        #[serde(default)]
        nodes: Vec<GraphNode>,
        #[serde(default)]
        edges: Vec<GraphEdge>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        highlight_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select_key: Option<String>,
    },
    Chart {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        #[serde(default)]
        chart_type: ChartType,
        #[serde(default)]
        x_labels: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x_unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y_unit: Option<String>,
        #[serde(default)]
        value_labels: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<String>,
        #[serde(default)]
        series: Vec<ChartSeries>,
    },
    Svg {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        source: String,
        #[serde(default)]
        tooltips: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        highlight_key: Option<String>,
    },
}

impl Node {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Split { .. } => None,
            Self::Scroll { id, .. }
            | Self::Tabs { id, .. }
            | Self::Log { id, .. }
            | Self::Graph { id, .. }
            | Self::Chart { id, .. }
            | Self::Svg { id, .. } => Some(id.as_str()),
            Self::Markdown { id, .. }
            | Self::Code { id, .. }
            | Self::Stat { id, .. }
            | Self::Table { id, .. } => id.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Event {
    Init(Box<Document>),
    SetState(StateMap),
    SetNode {
        graph_id: String,
        node_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sublabel: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<Status>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tooltip: Option<String>,
    },
    SetEdge {
        graph_id: String,
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tooltip: Option<String>,
    },
    SetStat {
        stat_id: String,
        item_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<Status>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sparkline_push: Option<f64>,
    },
    LogAppend {
        target: String,
        entries: Vec<LogEntry>,
    },
    ChartAppend {
        target: String,
        series: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x_label: Option<String>,
        points: Vec<ChartPoint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_points: Option<usize>,
    },
    Replace {
        target: String,
        node: Box<Node>,
    },
    Close,
}
