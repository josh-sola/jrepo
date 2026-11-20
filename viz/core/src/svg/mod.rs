pub mod chart;
pub mod graph;
pub mod hit_test;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use usvg::{Options, fontdb};

use crate::model::{Node, StateMap};
use crate::theme::Theme;

pub const INTER_TTF: &[u8] = include_bytes!("../../../assets/fonts/Inter.ttf");
pub const JETBRAINS_MONO_REGULAR_TTF: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
pub const JETBRAINS_MONO_BOLD_TTF: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Bold.ttf");

static FONTDB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

pub fn shared_fontdb() -> Arc<fontdb::Database> {
    Arc::clone(FONTDB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_font_data(INTER_TTF.to_vec());
        db.load_font_data(JETBRAINS_MONO_REGULAR_TTF.to_vec());
        db.load_font_data(JETBRAINS_MONO_BOLD_TTF.to_vec());
        db.set_sans_serif_family("Inter");
        db.set_monospace_family("JetBrains Mono");
        Arc::new(db)
    }))
}

pub fn usvg_options() -> Options<'static> {
    Options {
        fontdb: shared_fontdb(),
        font_family: "Inter".into(),
        ..Options::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitTransform {
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

pub fn compute_fit_transform(svg_w: f32, svg_h: f32, target_w: u32, target_h: u32) -> FitTransform {
    let tw = target_w.max(1) as f32;
    let th = target_h.max(1) as f32;
    if svg_w <= 0.0 || svg_h <= 0.0 {
        return FitTransform {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
        };
    }
    let scale = (tw / svg_w).min(th / svg_h);
    let offset_x = (tw - svg_w * scale) * 0.5;
    let offset_y = (th - svg_h * scale) * 0.5;
    FitTransform {
        scale,
        offset_x,
        offset_y,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HitTarget {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub tooltip: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartBucket {
    pub id: String,
    pub x_px: f32,
    pub x_min: f32,
    pub x_max: f32,
    pub x_label: String,
    pub values: Vec<(String, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HorizontalChartBucket {
    pub id: String,
    pub y_px: f32,
    pub y_min: f32,
    pub y_max: f32,
    pub y_label: String,
    pub values: Vec<(String, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScatterPoint {
    pub id: String,
    pub px: f32,
    pub py: f32,
    pub series: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChartHoverIndex {
    Buckets {
        plot_x: (f32, f32),
        plot_y: (f32, f32),
        buckets: Vec<ChartBucket>,
        y_unit: Option<String>,
    },
    HorizontalBuckets {
        plot_x: (f32, f32),
        plot_y: (f32, f32),
        buckets: Vec<HorizontalChartBucket>,
        unit: Option<String>,
    },
    Scatter {
        plot_x: (f32, f32),
        plot_y: (f32, f32),
        points: Vec<ScatterPoint>,
        y_unit: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoverHit {
    pub id: Option<String>,
    pub title: String,
    pub lines: Vec<String>,
    pub anchor_px: (f32, f32),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitMap {
    pub targets: Vec<HitTarget>,
    pub chart_hover: Option<ChartHoverIndex>,
}

impl HitMap {
    pub fn hit_at(&self, px: f32, py: f32) -> Option<HoverHit> {
        // Iterate in reverse so top-most / later-drawn targets win when boxes overlap.
        for target in self.targets.iter().rev() {
            if px >= target.x
                && px <= target.x + target.w
                && py >= target.y
                && py <= target.y + target.h
            {
                return Some(target_hover(target));
            }
        }

        let hover = self.chart_hover.as_ref()?;
        match hover {
            ChartHoverIndex::Buckets {
                plot_x,
                plot_y,
                buckets,
                y_unit,
            } => {
                if px < plot_x.0 || px > plot_x.1 || py < plot_y.0 || py > plot_y.1 {
                    return None;
                }
                let bucket = buckets
                    .iter()
                    .find(|b| px >= b.x_min && px <= b.x_max)
                    .or_else(|| {
                        buckets
                            .iter()
                            .min_by(|a, b| (a.x_px - px).abs().total_cmp(&(b.x_px - px).abs()))
                    })?;
                Some(bucket_hover(
                    &bucket.id,
                    &bucket.x_label,
                    &bucket.values,
                    y_unit.as_deref(),
                    (bucket.x_px, py.clamp(plot_y.0, plot_y.1)),
                ))
            }
            ChartHoverIndex::HorizontalBuckets {
                plot_x,
                plot_y,
                buckets,
                unit,
            } => {
                if px < plot_x.0 || px > plot_x.1 || py < plot_y.0 || py > plot_y.1 {
                    return None;
                }
                let bucket = buckets
                    .iter()
                    .find(|b| py >= b.y_min && py <= b.y_max)
                    .or_else(|| {
                        buckets
                            .iter()
                            .min_by(|a, b| (a.y_px - py).abs().total_cmp(&(b.y_px - py).abs()))
                    })?;
                Some(bucket_hover(
                    &bucket.id,
                    &bucket.y_label,
                    &bucket.values,
                    unit.as_deref(),
                    (px.clamp(plot_x.0, plot_x.1), bucket.y_px),
                ))
            }
            ChartHoverIndex::Scatter {
                plot_x,
                plot_y,
                points,
                y_unit,
            } => {
                if px < plot_x.0 || px > plot_x.1 || py < plot_y.0 || py > plot_y.1 {
                    return None;
                }
                const MAX_DIST_SQ: f32 = 24.0 * 24.0;
                let nearest = points
                    .iter()
                    .map(|pt| {
                        let dx = pt.px - px;
                        let dy = pt.py - py;
                        (pt, dx * dx + dy * dy)
                    })
                    .filter(|(_, d2)| *d2 <= MAX_DIST_SQ)
                    .min_by(|a, b| a.1.total_cmp(&b.1))?
                    .0;
                Some(scatter_hover(nearest, y_unit.as_deref()))
            }
        }
    }

    /// Later entries are drawn on top. Tooltips carry the TUI hover card text.
    pub fn hits(&self, target_kind: HitKind) -> Vec<Hit> {
        let mut hits: Vec<Hit> = self
            .targets
            .iter()
            .map(|t| Hit {
                id: t.id.clone(),
                kind: target_kind,
                x: t.x,
                y: t.y,
                w: t.w,
                h: t.h,
                tooltip: Some(target_hover(t).card_text()),
                selectable: true,
            })
            .collect();

        let bucket = |id: &str, (x, y, w, h): (f32, f32, f32, f32), hover: HoverHit| Hit {
            id: id.to_string(),
            kind: HitKind::Bucket,
            x,
            y,
            w,
            h,
            tooltip: Some(hover.card_text()),
            selectable: false,
        };
        match &self.chart_hover {
            None => {}
            Some(ChartHoverIndex::Buckets {
                plot_y,
                buckets,
                y_unit,
                ..
            }) => {
                let (top, bottom) = (plot_y.0.min(plot_y.1), plot_y.0.max(plot_y.1));
                for b in buckets {
                    let hover =
                        bucket_hover(&b.id, &b.x_label, &b.values, y_unit.as_deref(), (0.0, 0.0));
                    let rect = (b.x_min, top, b.x_max - b.x_min, bottom - top);
                    hits.push(bucket(&b.id, rect, hover));
                }
            }
            Some(ChartHoverIndex::HorizontalBuckets {
                plot_x,
                buckets,
                unit,
                ..
            }) => {
                let (left, right) = (plot_x.0.min(plot_x.1), plot_x.0.max(plot_x.1));
                for b in buckets {
                    let hover =
                        bucket_hover(&b.id, &b.y_label, &b.values, unit.as_deref(), (0.0, 0.0));
                    let rect = (left, b.y_min, right - left, b.y_max - b.y_min);
                    hits.push(bucket(&b.id, rect, hover));
                }
            }
            Some(ChartHoverIndex::Scatter { points, y_unit, .. }) => {
                const R: f32 = 8.0;
                for pt in points {
                    let rect = (pt.px - R, pt.py - R, R * 2.0, R * 2.0);
                    hits.push(bucket(&pt.id, rect, scatter_hover(pt, y_unit.as_deref())));
                }
            }
        }
        hits
    }

    pub fn transformed(&self, t: FitTransform) -> Self {
        let targets = self
            .targets
            .iter()
            .map(|target| HitTarget {
                id: target.id.clone(),
                x: target.x * t.scale + t.offset_x,
                y: target.y * t.scale + t.offset_y,
                w: target.w * t.scale,
                h: target.h * t.scale,
                tooltip: target.tooltip.clone(),
            })
            .collect();

        let chart_hover = self.chart_hover.as_ref().map(|hover| match hover {
            ChartHoverIndex::Buckets {
                plot_x,
                plot_y,
                buckets,
                y_unit,
            } => ChartHoverIndex::Buckets {
                plot_x: (
                    plot_x.0 * t.scale + t.offset_x,
                    plot_x.1 * t.scale + t.offset_x,
                ),
                plot_y: (
                    plot_y.0 * t.scale + t.offset_y,
                    plot_y.1 * t.scale + t.offset_y,
                ),
                buckets: buckets
                    .iter()
                    .map(|b| ChartBucket {
                        id: b.id.clone(),
                        x_px: b.x_px * t.scale + t.offset_x,
                        x_min: b.x_min * t.scale + t.offset_x,
                        x_max: b.x_max * t.scale + t.offset_x,
                        x_label: b.x_label.clone(),
                        values: b.values.clone(),
                    })
                    .collect(),
                y_unit: y_unit.clone(),
            },
            ChartHoverIndex::HorizontalBuckets {
                plot_x,
                plot_y,
                buckets,
                unit,
            } => ChartHoverIndex::HorizontalBuckets {
                plot_x: (
                    plot_x.0 * t.scale + t.offset_x,
                    plot_x.1 * t.scale + t.offset_x,
                ),
                plot_y: (
                    plot_y.0 * t.scale + t.offset_y,
                    plot_y.1 * t.scale + t.offset_y,
                ),
                buckets: buckets
                    .iter()
                    .map(|b| HorizontalChartBucket {
                        id: b.id.clone(),
                        y_px: b.y_px * t.scale + t.offset_y,
                        y_min: b.y_min * t.scale + t.offset_y,
                        y_max: b.y_max * t.scale + t.offset_y,
                        y_label: b.y_label.clone(),
                        values: b.values.clone(),
                    })
                    .collect(),
                unit: unit.clone(),
            },
            ChartHoverIndex::Scatter {
                plot_x,
                plot_y,
                points,
                y_unit,
            } => ChartHoverIndex::Scatter {
                plot_x: (
                    plot_x.0 * t.scale + t.offset_x,
                    plot_x.1 * t.scale + t.offset_x,
                ),
                plot_y: (
                    plot_y.0 * t.scale + t.offset_y,
                    plot_y.1 * t.scale + t.offset_y,
                ),
                points: points
                    .iter()
                    .map(|pt| ScatterPoint {
                        id: pt.id.clone(),
                        px: pt.px * t.scale + t.offset_x,
                        py: pt.py * t.scale + t.offset_y,
                        series: pt.series.clone(),
                        x: pt.x,
                        y: pt.y,
                    })
                    .collect(),
                y_unit: y_unit.clone(),
            },
        });

        Self {
            targets,
            chart_hover,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SvgBundle {
    pub svg: String,
    pub width: u32,
    pub height: u32,
    pub hit_map: HitMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HitKind {
    Node,
    Group,
    Edge,
    Element,
    Bucket,
}

/// One hoverable rectangle, in the same pixel space as the SVG it came with.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub id: String,
    pub kind: HitKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub tooltip: Option<String>,
    pub selectable: bool,
}

impl HoverHit {
    pub fn card_text(&self) -> String {
        let mut text = self.title.clone();
        for line in &self.lines {
            text.push('\n');
            text.push_str(line);
        }
        text
    }
}

fn target_hover(target: &HitTarget) -> HoverHit {
    let mut parts = target.tooltip.lines();
    let title = parts
        .next()
        .map(ToString::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| target.id.clone());
    let lines = parts.map(ToString::to_string).collect();
    HoverHit {
        id: Some(target.id.clone()),
        title,
        lines,
        anchor_px: (target.x + target.w * 0.5, target.y),
    }
}

fn bucket_hover(
    id: &str,
    label: &str,
    values: &[(String, f64)],
    unit: Option<&str>,
    anchor_px: (f32, f32),
) -> HoverHit {
    let suffix = unit.map(|u| format!(" {u}")).unwrap_or_default();
    let lines = values
        .iter()
        .map(|(name, v)| format!("{name}: {}{suffix}", fmt_metric(*v)))
        .collect();
    HoverHit {
        id: Some(id.to_string()),
        title: label.to_string(),
        lines,
        anchor_px,
    }
}

fn scatter_hover(point: &ScatterPoint, y_unit: Option<&str>) -> HoverHit {
    let suffix = y_unit.map(|u| format!(" {u}")).unwrap_or_default();
    HoverHit {
        id: Some(point.id.clone()),
        title: point.series.clone(),
        lines: vec![
            format!("x: {}", fmt_metric(point.x)),
            format!("y: {}{suffix}", fmt_metric(point.y)),
        ],
        anchor_px: (point.px, point.py),
    }
}

/// The pane's selected id and highlighted tokens.
pub fn selection<'a>(
    state: &'a StateMap,
    select_key: Option<&str>,
    highlight_key: Option<&str>,
) -> (Option<&'a str>, Vec<String>) {
    let selected = state
        .get(select_key.unwrap_or("selected"))
        .and_then(|v| v.as_str());
    let highlighted = state
        .get(highlight_key.unwrap_or("highlight"))
        .map(|v| v.as_list())
        .unwrap_or_default();
    (selected, highlighted)
}

/// Returns `None` unless `node` is a graph, chart, or svg. Charts get no title;
/// the pane header shows it.
pub fn render_node(
    node: &Node,
    state: &StateMap,
    scales: &BTreeMap<String, f64>,
    theme: &Theme,
    width: u32,
    height: u32,
) -> Option<Result<SvgBundle, String>> {
    match node {
        Node::Graph {
            direction,
            nodes,
            edges,
            highlight_key,
            select_key,
            ..
        } => {
            let (selected, highlighted) =
                selection(state, select_key.as_deref(), highlight_key.as_deref());
            Some(Ok(graph::render_graph_svg(
                &graph::GraphRenderInput {
                    nodes,
                    edges,
                    direction: *direction,
                    selected,
                    highlighted: &highlighted,
                },
                theme,
                width,
                height,
            )))
        }
        Node::Chart {
            id,
            chart_type,
            x_labels,
            x_unit,
            y_unit,
            value_labels,
            scale,
            series,
            ..
        } => Some(chart::render_chart_svg(
            &chart::ChartRenderInput {
                id,
                title: None,
                chart_type: *chart_type,
                x_labels,
                x_unit: x_unit.as_deref(),
                y_unit: y_unit.as_deref(),
                value_labels: *value_labels,
                max_override: scale.as_deref().and_then(|s| scales.get(s).copied()),
                series,
            },
            theme,
            width,
            height,
        )),
        Node::Svg {
            source,
            tooltips,
            select_key,
            highlight_key,
            ..
        } => {
            let (selected, highlighted) =
                selection(state, select_key.as_deref(), highlight_key.as_deref());
            Some(hit_test::prepare_custom_svg(
                source,
                tooltips,
                selected,
                &highlighted,
                theme,
                width,
                height,
            ))
        }
        _ => None,
    }
}

pub(crate) fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn fmt_metric(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{:.0}", v)
    } else if v.abs() >= 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}
