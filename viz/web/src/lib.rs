//! wasm-bindgen exports that let a browser normalize and draw viz documents.
//! Every export takes and returns strings (JSON in, JSON out) and throws a JS
//! `Error` with a readable message on failure.

use std::collections::BTreeMap;

use miette::Diagnostic;
use serde::Serialize;
use serde_json::json;
use viz_core::kdl_codec::parse_document;
use viz_core::model::{Document, Node, StateMap, VizShare};
use viz_core::store::{Store, compute_chart_scales};
use viz_core::svg::hit_test::fit_custom_svg;
use viz_core::svg::{Hit, HitKind, render_node};
use viz_core::theme::{Theme, ThemeColor};
use viz_core::timeline;
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
struct RenderOut {
    svg: String,
    width: u32,
    height: u32,
    hits: Vec<Hit>,
}

/// Parses KDL (v2 or v1) or JSON document text into canonical Document JSON.
#[wasm_bindgen]
pub fn normalize(text: &str) -> Result<String, JsError> {
    let doc = parse_document(text, "document").map_err(|e| JsError::new(&report_text(&e)))?;
    to_json(&doc)
}

/// The merged state for `step` (-1 for none) with `selected` ("" for none) on top.
#[wasm_bindgen]
pub fn effective_state(doc_json: &str, step: i32, selected: &str) -> Result<String, JsError> {
    let mut store = Store::new(from_json::<Document>(doc_json, "document")?);
    if let Ok(step) = usize::try_from(step) {
        store.set_step(step);
    }
    if !selected.is_empty() {
        store.select(Some(selected.to_string()));
    }
    to_json(&store.effective_state())
}

/// The shared `scale` group maxima, as `{name: max}`.
#[wasm_bindgen]
pub fn chart_scales(doc_json: &str) -> Result<String, JsError> {
    let doc = from_json::<Document>(doc_json, "document")?;
    to_json(&compute_chart_scales(&doc.root))
}

#[wasm_bindgen]
pub fn render_graph(
    node_json: &str,
    state_json: &str,
    theme: &str,
    width: f64,
    height: f64,
) -> Result<String, JsError> {
    let node = node_of_kind(node_json, "graph")?;
    let state = from_json::<StateMap>(state_json, "state")?;
    render(
        &node,
        &state,
        &BTreeMap::new(),
        theme,
        (width, height),
        HitKind::Node,
    )
}

#[wasm_bindgen]
pub fn render_chart(
    node_json: &str,
    scales_json: &str,
    theme: &str,
    width: f64,
    height: f64,
) -> Result<String, JsError> {
    let node = node_of_kind(node_json, "chart")?;
    let scales = from_json::<BTreeMap<String, f64>>(scales_json, "chart scales")?;
    render(
        &node,
        &StateMap::new(),
        &scales,
        theme,
        (width, height),
        HitKind::Bucket,
    )
}

#[wasm_bindgen]
pub fn render_svg(
    node_json: &str,
    state_json: &str,
    theme: &str,
    width: f64,
    height: f64,
) -> Result<String, JsError> {
    let node = node_of_kind(node_json, "svg")?;
    let state = from_json::<StateMap>(state_json, "state")?;
    render(
        &node,
        &state,
        &BTreeMap::new(),
        theme,
        (width, height),
        HitKind::Element,
    )
}

#[wasm_bindgen]
pub fn theme_tokens(theme: &str) -> Result<String, JsError> {
    let t = parse_theme(theme)?;
    let hex = |c: ThemeColor| c.hex.to_ascii_lowercase();
    to_json(&json!({
        "bg": hex(t.bg),
        "surface": hex(t.surface),
        "surface_alt": hex(t.surface_alt),
        "border": hex(t.border),
        "fg": hex(t.fg),
        "muted": hex(t.muted),
        "accent": hex(t.accent),
        "selection": hex(t.selection),
        "highlight": hex(t.highlight),
        "ok": hex(t.ok),
        "warn": hex(t.warn),
        "error": hex(t.error),
        "active": hex(t.active),
        "idle": hex(t.idle),
        "status_muted": hex(t.status_muted),
        "chart_palette": t.chart_palette.iter().map(|c| hex(*c)).collect::<Vec<_>>(),
    }))
}

#[wasm_bindgen]
pub struct Timeline(timeline::Timeline);

#[wasm_bindgen]
impl Timeline {
    #[wasm_bindgen(constructor)]
    pub fn new(share_json: &str) -> Result<Timeline, JsError> {
        let share = from_json::<VizShare>(share_json, "share")?;
        if share.v != 1 {
            return Err(JsError::new(&format!(
                "unsupported share version `{}`",
                share.v
            )));
        }
        Ok(Self(timeline::Timeline::new(share)))
    }

    pub fn positions(&self) -> u32 {
        self.0.positions() as u32
    }

    /// `{doc, step}`: the raw document at position `i` (base state, not merged
    /// with step state) and the step index, or -1 when it has no steps.
    pub fn at(&self, i: u32) -> Result<String, JsError> {
        let store = self.0.at(i as usize).ok_or_else(|| self.out_of_range(i))?;
        let step = if store.doc.steps.is_empty() {
            -1
        } else {
            store.step_index as i64
        };
        to_json(&json!({ "doc": store.doc, "step": step }))
    }

    /// `{ts, summary}` for position `i`.
    pub fn label(&self, i: u32) -> Result<String, JsError> {
        let (ts, summary) = self
            .0
            .label(i as usize)
            .ok_or_else(|| self.out_of_range(i))?;
        to_json(&json!({ "ts": ts, "summary": summary }))
    }

    fn out_of_range(&self, i: u32) -> JsError {
        JsError::new(&format!(
            "timeline position {i} is out of range (0..{})",
            self.0.positions()
        ))
    }
}

fn render(
    node: &Node,
    state: &StateMap,
    scales: &BTreeMap<String, f64>,
    theme: &str,
    (width, height): (f64, f64),
    kind: HitKind,
) -> Result<String, JsError> {
    let theme = parse_theme(theme)?;
    let bundle = render_node(node, state, scales, &theme, px(width), px(height))
        .expect("callers pass a graph, chart, or svg node")
        .map_err(|e| JsError::new(&e))?;
    let svg = if matches!(node, Node::Svg { .. }) {
        fit_custom_svg(&bundle.svg, bundle.width, bundle.height).map_err(|e| JsError::new(&e))?
    } else {
        bundle.svg
    };
    to_json(&RenderOut {
        svg,
        width: bundle.width,
        height: bundle.height,
        hits: bundle.hit_map.hits(kind),
    })
}

fn node_of_kind(node_json: &str, kind: &str) -> Result<Node, JsError> {
    let node = from_json::<Node>(node_json, "node")?;
    let actual = match &node {
        Node::Graph { .. } => "graph",
        Node::Chart { .. } => "chart",
        Node::Svg { .. } => "svg",
        _ => "non-graphic",
    };
    if actual != kind {
        return Err(JsError::new(&format!(
            "expected a {kind} node, got a {actual} node"
        )));
    }
    Ok(node)
}

fn parse_theme(theme: &str) -> Result<Theme, JsError> {
    match theme {
        "light" => Ok(Theme::light()),
        "dark" => Ok(Theme::dark()),
        other => Err(JsError::new(&format!(
            "unknown theme `{other}`; expected \"light\" or \"dark\""
        ))),
    }
}

fn px(v: f64) -> u32 {
    if v.is_finite() && v > 0.0 {
        v.round().min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

fn from_json<T: serde::de::DeserializeOwned>(text: &str, what: &str) -> Result<T, JsError> {
    serde_json::from_str(text).map_err(|e| JsError::new(&format!("invalid {what} JSON: {e}")))
}

fn to_json<T: Serialize>(value: &T) -> Result<String, JsError> {
    serde_json::to_string(value).map_err(|e| JsError::new(&format!("failed to encode JSON: {e}")))
}

fn report_text(report: &miette::Report) -> String {
    let diag: &dyn Diagnostic = report.as_ref();
    let mut out = diag.to_string();
    if let (Some(src), Some(label)) = (diag.source_code(), diag.labels().and_then(|mut l| l.next()))
        && let Ok(span) = src.read_span(label.inner(), 0, 0)
    {
        out.push_str(&format!(
            " (line {}, column {})",
            span.line() + 1,
            span.column() + 1
        ));
        if let Some(text) = label.label() {
            out.push_str(&format!(": {text}"));
        }
    }
    if let Some(help) = diag.help() {
        out.push_str(&format!("\nhelp: {help}"));
    }
    out
}
