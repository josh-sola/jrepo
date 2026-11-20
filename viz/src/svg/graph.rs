use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use rust_sugiyama::configure::Config;

use crate::model::{EdgeStyle, GraphDirection, GraphEdge, GraphNode, NodeShape};
use crate::svg::{HitMap, HitTarget, SvgBundle, escape_xml};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy)]
struct CardRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl CardRect {
    fn cx(self) -> f32 {
        self.x + self.w * 0.5
    }

    fn cy(self) -> f32 {
        self.y + self.h * 0.5
    }
}

pub struct GraphRenderInput<'a> {
    pub nodes: &'a [GraphNode],
    pub edges: &'a [GraphEdge],
    pub direction: GraphDirection,
    pub selected: Option<&'a str>,
    pub highlighted: &'a [String],
}

pub fn render_graph_svg(
    input: &GraphRenderInput<'_>,
    theme: &Theme,
    target_w: u32,
    target_h: u32,
) -> SvgBundle {
    let nodes = input.nodes;
    let edges = input.edges;
    let direction = input.direction;
    let selected = input.selected;
    let highlighted = input.highlighted;
    let tw = target_w.max(64);
    let th = target_h.max(64);

    if nodes.is_empty() {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{tw}" height="{th}" viewBox="0 0 {tw} {th}"><rect width="{tw}" height="{th}" fill="{bg}"/><text x="{cx}" y="{cy}" text-anchor="middle" dominant-baseline="middle" font-family="Inter, sans-serif" font-size="14" fill="{muted}">Empty graph</text></svg>"##,
            bg = theme.bg.hex,
            cx = tw / 2,
            cy = th / 2,
            muted = theme.muted.hex,
        );
        return SvgBundle {
            svg,
            width: tw,
            height: th,
            hit_map: HitMap::default(),
        };
    }

    let mut graph: StableDiGraph<usize, usize> = StableDiGraph::new();
    let mut id_to_idx: HashMap<&str, NodeIndex> = HashMap::with_capacity(nodes.len());
    let mut card_sizes: Vec<(f64, f64)> = Vec::with_capacity(nodes.len());

    for (i, node) in nodes.iter().enumerate() {
        let idx = graph.add_node(i);
        id_to_idx.insert(node.id.as_str(), idx);
        card_sizes.push(measure_card(node));
    }

    for (edge_idx, edge) in edges.iter().enumerate() {
        if edge.from == edge.to {
            continue;
        }
        if let (Some(&u), Some(&v)) = (
            id_to_idx.get(edge.from.as_str()),
            id_to_idx.get(edge.to.as_str()),
        ) {
            graph.add_edge(u, v, edge_idx);
        }
    }

    let id_to_pos: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    let margin = 24.0f32;
    let avail_w = (tw as f32 - margin * 2.0).max(16.0);
    let avail_h = (th as f32 - margin * 2.0).max(16.0);
    let fit_scale = |layout: &GraphLayout<'_>| -> f32 {
        (avail_w / layout.span_w)
            .min(avail_h / layout.span_h)
            .min(2.0)
    };

    let mut effective_direction = direction;
    let mut layout = compute_layout(
        &graph,
        nodes,
        edges,
        &id_to_pos,
        &card_sizes,
        effective_direction,
    );
    let mut scale = fit_scale(&layout);

    if direction == GraphDirection::Lr && scale < 0.72 {
        let tb_layout = compute_layout(
            &graph,
            nodes,
            edges,
            &id_to_pos,
            &card_sizes,
            GraphDirection::Tb,
        );
        let tb_scale = fit_scale(&tb_layout);
        if tb_scale > scale * 1.25 {
            layout = tb_layout;
            scale = tb_scale;
            effective_direction = GraphDirection::Tb;
        }
    }

    let raw_rects = layout.raw_rects;
    let group_boxes = layout.group_boxes;
    let offset_x = (tw as f32 - layout.span_w * scale) * 0.5 - layout.min_x * scale;
    let offset_y = (th as f32 - layout.span_h * scale) * 0.5 - layout.min_y * scale;

    let map_rect = |r: CardRect| CardRect {
        x: r.x * scale + offset_x,
        y: r.y * scale + offset_y,
        w: r.w * scale,
        h: r.h * scale,
    };

    let rects: Vec<CardRect> = (0..nodes.len()).map(|i| map_rect(raw_rects[&i])).collect();

    let mut svg = String::with_capacity(4096);
    let _ = writeln!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{tw}" height="{th}" viewBox="0 0 {tw} {th}">"##
    );
    let _ = writeln!(
        svg,
        r##"<defs>
  <marker id="arrow-muted" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
    <path d="M 0 1 L 10 5 L 0 9 z" fill="{muted}"/>
  </marker>
  <marker id="arrow-active" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
    <path d="M 0 1 L 10 5 L 0 9 z" fill="{accent}"/>
  </marker>
</defs>"##,
        muted = theme.muted.hex,
        accent = theme.accent.hex,
    );
    let _ = writeln!(
        svg,
        r##"<rect x="0" y="0" width="{tw}" height="{th}" fill="{bg}"/>"##,
        bg = theme.bg.hex
    );

    let has_lit = !highlighted.is_empty();
    let mut lit_nodes: HashSet<&str> = HashSet::new();
    let mut lit_groups: HashSet<&str> = HashSet::new();
    let mut lit_edges: HashSet<(&str, &str)> = HashSet::new();
    if has_lit {
        if let Some(sel) = selected {
            lit_nodes.insert(sel);
        }
        for token in highlighted {
            if let Some((from, to)) = token.split_once("->") {
                let from = from.trim();
                let to = to.trim();
                lit_edges.insert((from, to));
                lit_nodes.insert(from);
                lit_nodes.insert(to);
                continue;
            }
            if id_to_pos.contains_key(token.as_str()) {
                lit_nodes.insert(token.as_str());
            }
            let mut matched_group = false;
            for n in nodes {
                if n.group.as_deref() == Some(token.as_str()) {
                    lit_nodes.insert(n.id.as_str());
                    matched_group = true;
                }
            }
            if matched_group {
                lit_groups.insert(token.as_str());
            }
        }
    }

    for (group_name, raw_gb) in &group_boxes {
        let gb = map_rect(*raw_gb);
        let font_sz = (11.0 * scale).clamp(9.0, 14.0);
        let group_lit = !has_lit
            || lit_groups.contains(group_name)
            || nodes.iter().any(|n| {
                n.group.as_deref() == Some(*group_name) && lit_nodes.contains(n.id.as_str())
            });
        let opacity_attr = if group_lit { "" } else { r#" opacity="0.42""# };
        let _ = writeln!(
            svg,
            r##"<g id="group:{gid}"{opacity_attr}>
  <rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="12" fill="{fill}" fill-opacity="0.45" stroke="{border}" stroke-width="1.2" stroke-dasharray="6,4"/>
  <text x="{tx:.1}" y="{ty:.1}" font-family="Inter, sans-serif" font-size="{font_sz:.1}" font-weight="600" fill="{muted}">{label}</text>
</g>"##,
            gid = escape_xml(group_name),
            x = gb.x,
            y = gb.y,
            w = gb.w,
            h = gb.h,
            fill = theme.surface_alt.hex,
            border = theme.border.hex,
            tx = gb.x + 12.0 * scale,
            ty = gb.y + 18.0 * scale,
            muted = theme.muted.hex,
            label = escape_xml(group_name),
        );
    }

    let mut hit_targets = Vec::with_capacity(nodes.len() + edges.len());

    for edge in edges {
        let (Some(&u_pos), Some(&v_pos)) = (
            id_to_pos.get(edge.from.as_str()),
            id_to_pos.get(edge.to.as_str()),
        ) else {
            continue;
        };
        let u_rect = rects[u_pos];
        let v_rect = rects[v_pos];

        let is_explicit_lit = lit_edges.contains(&(edge.from.as_str(), edge.to.as_str()));
        let edge_lit = !has_lit
            || is_explicit_lit
            || (lit_nodes.contains(edge.from.as_str()) && lit_nodes.contains(edge.to.as_str()));

        let stroke = if edge.active || is_explicit_lit {
            theme.accent.hex
        } else {
            theme.muted.hex
        };
        let stroke_w = if edge.active || is_explicit_lit {
            2.4
        } else {
            1.6
        };
        let marker = if edge.active || is_explicit_lit {
            "url(#arrow-active)"
        } else {
            "url(#arrow-muted)"
        };
        let dash = match edge.style {
            EdgeStyle::Solid => "",
            EdgeStyle::Dashed => r#" stroke-dasharray="6,4""#,
        };

        let (path_d, mid_x, mid_y) = if u_pos == v_pos {
            let x1 = u_rect.x + u_rect.w * 0.7;
            let y1 = u_rect.y;
            let x2 = u_rect.x + u_rect.w * 0.3;
            let y2 = u_rect.y;
            let loop_h = 36.0 * scale;
            (
                format!(
                    "M {x1:.1} {y1:.1} C {c1x:.1} {cy:.1}, {c2x:.1} {cy:.1}, {x2:.1} {y2:.1}",
                    c1x = x1 + 20.0 * scale,
                    c2x = x2 - 20.0 * scale,
                    cy = y1 - loop_h,
                ),
                u_rect.cx(),
                y1 - loop_h * 0.72,
            )
        } else {
            let (x1, y1, x2, y2, c1x, c1y, c2x, c2y) =
                edge_bezier(u_rect, v_rect, effective_direction);
            let mx = 0.125 * x1 + 0.375 * c1x + 0.375 * c2x + 0.125 * x2;
            let my = 0.125 * y1 + 0.375 * c1y + 0.375 * c2y + 0.125 * y2;
            (
                format!(
                    "M {x1:.1} {y1:.1} C {c1x:.1} {c1y:.1}, {c2x:.1} {c2y:.1}, {x2:.1} {y2:.1}"
                ),
                mx,
                my,
            )
        };

        let edge_opacity_attr = if edge_lit { "" } else { r#" opacity="0.35""# };
        let _ = writeln!(svg, "<g{edge_opacity_attr}>");
        let _ = writeln!(
            svg,
            r##"  <path d="{path_d}" fill="none" stroke="{stroke}" stroke-width="{stroke_w:.1}"{dash} marker-end="{marker}"/>"##
        );

        if let Some(label) = &edge.label {
            let font_sz = (11.0 * scale).clamp(6.0, 14.0);
            let pill_w =
                (label.chars().count() as f32 * font_sz * 0.62 + 10.0 * scale).max(24.0 * scale);
            let pill_h = font_sz + 6.0 * scale;
            let _ = writeln!(
                svg,
                r##"  <rect x="{px:.1}" y="{py:.1}" width="{pill_w:.1}" height="{pill_h:.1}" rx="4" fill="{bg}" stroke="{border}" stroke-width="1"/>
  <text x="{mid_x:.1}" y="{ty:.1}" text-anchor="middle" dominant-baseline="middle" font-family="Inter, sans-serif" font-size="{font_sz:.1}" fill="{fg}">{text}</text>"##,
                px = mid_x - pill_w * 0.5,
                py = mid_y - pill_h * 0.5,
                bg = theme.surface.hex,
                border = theme.border.hex,
                ty = mid_y + 0.5,
                fg = if edge.active || is_explicit_lit {
                    theme.accent.hex
                } else {
                    theme.muted.hex
                },
                text = escape_xml(label),
            );
        }
        svg.push_str("</g>\n");

        if let Some(tip) = &edge.tooltip {
            let edge_id = format!("edge:{}->{}", edge.from, edge.to);
            let mut full_tip = format!("{} -> {}", edge.from, edge.to);
            if let Some(lbl) = &edge.label {
                let _ = write!(full_tip, " ({lbl})");
            }
            let _ = write!(full_tip, "\n{tip}");
            hit_targets.push(HitTarget {
                id: edge_id,
                x: mid_x - 18.0,
                y: mid_y - 12.0,
                w: 36.0,
                h: 24.0,
                tooltip: full_tip,
            });
        }
    }

    for (i, node) in nodes.iter().enumerate() {
        let r = rects[i];
        let is_selected = selected == Some(node.id.as_str());
        let is_highlighted = lit_nodes.contains(node.id.as_str());
        let node_lit = !has_lit || is_highlighted;
        let opacity_attr = if node_lit { "" } else { r#" opacity="0.42""# };

        let fill = if is_selected {
            theme.selection.hex
        } else {
            theme.surface.hex
        };
        let (stroke, stroke_w) = if is_selected {
            (theme.accent.hex, 2.8)
        } else if is_highlighted {
            (theme.highlight.hex, 2.4)
        } else if let Some(st) = node.status {
            (theme.status_color(st).hex, 1.6)
        } else {
            (theme.border.hex, 1.4)
        };

        let _ = writeln!(
            svg,
            r#"<g id="node:{}"{opacity_attr}>"#,
            escape_xml(&node.id)
        );
        match node.shape {
            NodeShape::Rounded => {
                let _ = writeln!(
                    svg,
                    r#"  <rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="10" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_w:.1}"/>"#,
                    x = r.x,
                    y = r.y,
                    w = r.w,
                    h = r.h,
                );
            }
            NodeShape::Box => {
                let _ = writeln!(
                    svg,
                    r#"  <rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="2" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_w:.1}"/>"#,
                    x = r.x,
                    y = r.y,
                    w = r.w,
                    h = r.h,
                );
            }
            NodeShape::Pill => {
                let rx = r.h * 0.5;
                let _ = writeln!(
                    svg,
                    r#"  <rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="{rx:.1}" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_w:.1}"/>"#,
                    x = r.x,
                    y = r.y,
                    w = r.w,
                    h = r.h,
                );
            }
            NodeShape::Diamond => {
                let pts = format!(
                    "{:.1},{:.1} {:.1},{:.1} {:.1},{:.1} {:.1},{:.1}",
                    r.cx(),
                    r.y,
                    r.x + r.w,
                    r.cy(),
                    r.cx(),
                    r.y + r.h,
                    r.x,
                    r.cy()
                );
                let _ = writeln!(
                    svg,
                    r#"  <polygon points="{pts}" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_w:.1}"/>"#
                );
            }
        }

        if let Some(st) = node.status {
            let dot_r = (4.5 * scale).clamp(3.0, 6.5);
            let (dot_x, dot_y) = match node.shape {
                NodeShape::Diamond => (r.cx(), r.y + r.h * 0.21),
                NodeShape::Pill => (r.x + r.h * 0.36, r.cy()),
                NodeShape::Rounded | NodeShape::Box => (r.x + 14.0 * scale, r.y + 14.0 * scale),
            };
            let _ = writeln!(
                svg,
                r#"  <circle cx="{dot_x:.1}" cy="{dot_y:.1}" r="{dot_r:.1}" fill="{col}"/>"#,
                col = theme.status_color(st).hex,
            );
        }

        let label_sz = (13.0 * scale).clamp(6.5, 18.0);
        let sub_sz = (10.5 * scale).clamp(5.5, 14.5);
        let label_y = if node.sublabel.is_some() {
            r.cy() - sub_sz * 0.55
        } else {
            r.cy() + 1.0
        };
        let _ = writeln!(
            svg,
            r#"  <text x="{cx:.1}" y="{label_y:.1}" text-anchor="middle" dominant-baseline="middle" font-family="Inter, sans-serif" font-size="{label_sz:.1}" font-weight="600" fill="{fg}">{label}</text>"#,
            cx = r.cx(),
            fg = theme.fg.hex,
            label = escape_xml(&node.label),
        );
        if let Some(sub) = &node.sublabel {
            let sub_y = r.cy() + label_sz * 0.65;
            let _ = writeln!(
                svg,
                r#"  <text x="{cx:.1}" y="{sub_y:.1}" text-anchor="middle" dominant-baseline="middle" font-family="JetBrains Mono, monospace" font-size="{sub_sz:.1}" fill="{muted}">{sub}</text>"#,
                cx = r.cx(),
                muted = theme.muted.hex,
                sub = escape_xml(sub),
            );
        }
        svg.push_str("</g>\n");

        let mut tip = node.label.clone();
        if let Some(st) = node.status {
            let _ = write!(tip, " [{}]", st.as_str());
        }
        if let Some(sub) = &node.sublabel {
            let _ = write!(tip, "\n{sub}");
        }
        if let Some(extra) = &node.tooltip {
            let _ = write!(tip, "\n{extra}");
        }
        hit_targets.push(HitTarget {
            id: node.id.clone(),
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            tooltip: tip,
        });
    }

    svg.push_str("</svg>\n");

    SvgBundle {
        svg,
        width: tw,
        height: th,
        hit_map: HitMap {
            targets: hit_targets,
            chart_hover: None,
        },
    }
}

struct GraphLayout<'a> {
    raw_rects: HashMap<usize, CardRect>,
    group_boxes: BTreeMap<&'a str, CardRect>,
    min_x: f32,
    min_y: f32,
    span_w: f32,
    span_h: f32,
}

fn compute_layout<'a>(
    graph: &StableDiGraph<usize, usize>,
    nodes: &'a [GraphNode],
    edges: &[GraphEdge],
    id_to_pos: &HashMap<&str, usize>,
    card_sizes: &[(f64, f64)],
    direction: GraphDirection,
) -> GraphLayout<'a> {
    let min_sibling_gap = 44.0f32;
    let config = Config {
        vertex_spacing: 76.0,
        minimum_length: 1,
        ..Config::default()
    };

    // rust-sugiyama lays out top-to-bottom, so for LR we swap vertex width/height
    // before layout and transpose the returned (x, y) coordinates afterward.
    let layouts = rust_sugiyama::from_graph(
        graph,
        &|_idx, &node_pos| {
            let (w, h) = card_sizes[node_pos];
            match direction {
                GraphDirection::Tb => (w, h),
                GraphDirection::Lr => (h, w),
            }
        },
        &config,
    );

    let mut raw_rects: HashMap<usize, CardRect> = HashMap::with_capacity(nodes.len());
    let component_gap = 72.0f32;
    let mut cursor = 0.0f32;

    for (positions, _w, _h) in layouts {
        if positions.is_empty() {
            continue;
        }

        // Bucket vertices by Sugiyama rank (ly) so we can enforce edge-label and group clearance.
        let mut by_rank: BTreeMap<i64, Vec<(usize, f32)>> = BTreeMap::new();
        for (node_idx, (lx, ly)) in positions {
            let pos = graph[node_idx];
            by_rank
                .entry(ly.round() as i64)
                .or_default()
                .push((pos, lx as f32));
        }

        let mut comp_rects: Vec<(usize, CardRect)> = Vec::new();
        let mut rank_cursor = 0.0f32;
        let mut prev_groups: Vec<Option<&str>> = Vec::new();
        let mut prev_rank_nodes: HashSet<usize> = HashSet::new();

        for mut rank_nodes in by_rank.into_values() {
            rank_nodes.sort_by(|a, b| a.1.total_cmp(&b.1));

            let curr_rank_nodes: HashSet<usize> = rank_nodes.iter().map(|&(pos, _)| pos).collect();
            let rank_groups: Vec<Option<&str>> = rank_nodes
                .iter()
                .map(|(pos, _)| nodes[*pos].group.as_deref())
                .collect();

            if !prev_groups.is_empty() {
                let crosses_group = rank_groups.iter().any(|g| g.is_some())
                    && prev_groups.iter().any(|g| g.is_some())
                    && !rank_groups
                        .iter()
                        .any(|g| g.is_some() && prev_groups.contains(g));

                let rank_gap = match direction {
                    GraphDirection::Tb => 42.0 + if crosses_group { 20.0 } else { 0.0 },
                    GraphDirection::Lr => {
                        let pair_max_label_w = edges
                            .iter()
                            .filter_map(|e| {
                                let label = e.label.as_deref()?;
                                let u = *id_to_pos.get(e.from.as_str())?;
                                let v = *id_to_pos.get(e.to.as_str())?;
                                let connects_pair = (prev_rank_nodes.contains(&u)
                                    && curr_rank_nodes.contains(&v))
                                    || (prev_rank_nodes.contains(&v)
                                        && curr_rank_nodes.contains(&u));
                                connects_pair.then(|| (label.chars().count() as f32) * 7.2 + 24.0)
                            })
                            .fold(0.0f32, f32::max);
                        (pair_max_label_w + 36.0).max(88.0) + if crosses_group { 24.0 } else { 0.0 }
                    }
                };
                rank_cursor += rank_gap;
            }

            let rank_depth = rank_nodes
                .iter()
                .map(|(pos, _)| match direction {
                    GraphDirection::Lr => card_sizes[*pos].0 as f32,
                    GraphDirection::Tb => card_sizes[*pos].1 as f32,
                })
                .fold(0.0f32, f32::max);

            // Space siblings within the rank with guaranteed clearance and center them around 0.
            let mut cross_positions: Vec<(usize, f32, f32)> = Vec::with_capacity(rank_nodes.len());
            let mut cross_cursor = 0.0f32;
            for (idx, &(pos, _)) in rank_nodes.iter().enumerate() {
                let cross_len = match direction {
                    GraphDirection::Lr => card_sizes[pos].1 as f32,
                    GraphDirection::Tb => card_sizes[pos].0 as f32,
                };
                if idx > 0 {
                    let prev_pos = rank_nodes[idx - 1].0;
                    let diff_group = nodes[prev_pos].group != nodes[pos].group
                        && (nodes[prev_pos].group.is_some() || nodes[pos].group.is_some());
                    let gap = min_sibling_gap + if diff_group { 36.0 } else { 0.0 };
                    cross_cursor += gap;
                }
                cross_positions.push((pos, cross_cursor, cross_len));
                cross_cursor += cross_len;
            }
            let total_cross = cross_cursor;
            let cross_shift = -total_cross * 0.5;

            for (pos, cross_start, _cross_len) in cross_positions {
                let (cw, ch) = (card_sizes[pos].0 as f32, card_sizes[pos].1 as f32);
                let rect = match direction {
                    GraphDirection::Lr => CardRect {
                        x: rank_cursor + (rank_depth - cw) * 0.5,
                        y: cross_start + cross_shift,
                        w: cw,
                        h: ch,
                    },
                    GraphDirection::Tb => CardRect {
                        x: cross_start + cross_shift,
                        y: rank_cursor + (rank_depth - ch) * 0.5,
                        w: cw,
                        h: ch,
                    },
                };
                comp_rects.push((pos, rect));
            }

            rank_cursor += rank_depth;
            prev_groups = rank_groups;
            prev_rank_nodes = curr_rank_nodes;
        }

        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (_, rect) in &comp_rects {
            min_x = min_x.min(rect.x);
            min_y = min_y.min(rect.y);
            max_x = max_x.max(rect.x + rect.w);
            max_y = max_y.max(rect.y + rect.h);
        }

        let comp_w = (max_x - min_x).max(1.0);
        let comp_h = (max_y - min_y).max(1.0);

        for (pos, mut rect) in comp_rects {
            match direction {
                GraphDirection::Tb => {
                    rect.x = rect.x - min_x + cursor;
                    rect.y -= min_y;
                }
                GraphDirection::Lr => {
                    rect.x -= min_x;
                    rect.y = rect.y - min_y + cursor;
                }
            }
            raw_rects.insert(pos, rect);
        }

        cursor += match direction {
            GraphDirection::Tb => comp_w + component_gap,
            GraphDirection::Lr => comp_h + component_gap,
        };
    }

    for (i, &(cw, ch)) in card_sizes.iter().enumerate() {
        raw_rects.entry(i).or_insert(CardRect {
            x: cursor,
            y: 0.0,
            w: cw as f32,
            h: ch as f32,
        });
    }

    let mut group_boxes: BTreeMap<&'a str, CardRect> = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let Some(group) = node.group.as_deref() else {
            continue;
        };
        let r = raw_rects[&i];
        let pad_x = 14.0f32;
        let pad_top = 22.0f32;
        let pad_bottom = 10.0f32;
        group_boxes
            .entry(group)
            .and_modify(|gb| {
                let x1 = gb.x.min(r.x - pad_x);
                let y1 = gb.y.min(r.y - pad_top);
                let x2 = (gb.x + gb.w).max(r.x + r.w + pad_x);
                let y2 = (gb.y + gb.h).max(r.y + r.h + pad_bottom);
                *gb = CardRect {
                    x: x1,
                    y: y1,
                    w: x2 - x1,
                    h: y2 - y1,
                };
            })
            .or_insert(CardRect {
                x: r.x - pad_x,
                y: r.y - pad_top,
                w: r.w + pad_x * 2.0,
                h: r.h + pad_top + pad_bottom,
            });
    }

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for r in raw_rects.values().chain(group_boxes.values()) {
        min_x = min_x.min(r.x);
        min_y = min_y.min(r.y);
        max_x = max_x.max(r.x + r.w);
        max_y = max_y.max(r.y + r.h);
    }

    GraphLayout {
        raw_rects,
        group_boxes,
        min_x,
        min_y,
        span_w: (max_x - min_x).max(1.0),
        span_h: (max_y - min_y).max(1.0),
    }
}

fn measure_card(node: &GraphNode) -> (f64, f64) {
    let label_w = (node.label.chars().count() as f64) * 8.6;
    let sub_w = node
        .sublabel
        .as_deref()
        .map_or(0.0, |s| (s.chars().count() as f64) * 6.8);
    let text_w = label_w.max(sub_w) + 48.0;
    let h = if node.sublabel.is_some() { 50.0 } else { 42.0 };
    match node.shape {
        NodeShape::Diamond => {
            // Diamond inscribed rectangle is half width/height, so expand bounding geometry to keep text inside.
            let w = text_w.clamp(136.0, 280.0);
            (w * 1.50, h * 1.38)
        }
        NodeShape::Pill => {
            // Semicircular endcaps reduce usable horizontal width by roughly h, so add endcap clearance.
            let w = (text_w + h * 0.75).clamp(136.0, 360.0);
            (w, h)
        }
        NodeShape::Rounded | NodeShape::Box => (text_w.clamp(136.0, 300.0), h),
    }
}

fn edge_bezier(
    u: CardRect,
    v: CardRect,
    direction: GraphDirection,
) -> (f32, f32, f32, f32, f32, f32, f32, f32) {
    match direction {
        GraphDirection::Lr => {
            if u.cx() <= v.cx() {
                let x1 = u.x + u.w;
                let y1 = u.cy();
                let x2 = v.x;
                let y2 = v.cy();
                let dx = ((x2 - x1).abs() * 0.45).max(24.0);
                (x1, y1, x2, y2, x1 + dx, y1, x2 - dx, y2)
            } else {
                // Back-edge (cycle): arc smoothly over the cards so it stays legible.
                let x1 = u.cx();
                let y1 = u.y;
                let x2 = v.cx();
                let y2 = v.y;
                let lift = ((x1 - x2).abs() * 0.25).clamp(32.0, 96.0);
                let top = y1.min(y2) - lift;
                (x1, y1, x2, y2, x1, top, x2, top)
            }
        }
        GraphDirection::Tb => {
            if u.cy() <= v.cy() {
                let x1 = u.cx();
                let y1 = u.y + u.h;
                let x2 = v.cx();
                let y2 = v.y;
                let dy = ((y2 - y1).abs() * 0.45).max(24.0);
                (x1, y1, x2, y2, x1, y1 + dy, x2, y2 - dy)
            } else {
                let x1 = u.x + u.w;
                let y1 = u.cy();
                let x2 = v.x + v.w;
                let y2 = v.cy();
                let bend = ((y1 - y2).abs() * 0.25).clamp(32.0, 96.0);
                let right = x1.max(x2) + bend;
                (x1, y1, x2, y2, right, y1, right, y2)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    fn sample_nodes_and_edges() -> (Vec<GraphNode>, Vec<GraphEdge>) {
        let nodes = vec![
            GraphNode {
                id: "gw".into(),
                label: "Gateway".into(),
                sublabel: Some("envoy".into()),
                status: Some(Status::Ok),
                group: Some("edge".into()),
                shape: NodeShape::Rounded,
                tooltip: Some("Handles TLS".into()),
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
            GraphNode {
                id: "db".into(),
                label: "Postgres".into(),
                sublabel: Some("primary".into()),
                status: Some(Status::Warn),
                group: Some("core".into()),
                shape: NodeShape::Pill,
                tooltip: None,
            },
            GraphNode {
                id: "orphan".into(),
                label: "Cron".into(),
                sublabel: None,
                status: None,
                group: None,
                shape: NodeShape::Box,
                tooltip: None,
            },
        ];
        let edges = vec![
            GraphEdge {
                from: "gw".into(),
                to: "auth".into(),
                label: Some("gRPC".into()),
                style: EdgeStyle::Solid,
                active: true,
                tooltip: Some("mTLS hop".into()),
            },
            GraphEdge {
                from: "auth".into(),
                to: "db".into(),
                label: None,
                style: EdgeStyle::Dashed,
                active: false,
                tooltip: None,
            },
            GraphEdge {
                from: "db".into(),
                to: "gw".into(),
                label: Some("invalidate".into()),
                style: EdgeStyle::Dashed,
                active: false,
                tooltip: Some("Cache bust".into()),
            },
        ];
        (nodes, edges)
    }

    #[test]
    fn renders_lr_and_tb_with_groups_cycles_and_hit_targets() {
        let (nodes, edges) = sample_nodes_and_edges();
        let highlighted = vec!["auth".into()];
        for dir in [GraphDirection::Lr, GraphDirection::Tb] {
            let bundle = render_graph_svg(
                &GraphRenderInput {
                    nodes: &nodes,
                    edges: &edges,
                    direction: dir,
                    selected: Some("gw"),
                    highlighted: &highlighted,
                },
                &Theme::dark(),
                800,
                500,
            );
            assert!(bundle.svg.contains(r#"id="node:gw""#));
            assert!(bundle.svg.contains(r#"id="group:core""#));

            let gw_target = bundle
                .hit_map
                .targets
                .iter()
                .find(|t| t.id == "gw")
                .expect("gateway hit target");
            let hit = bundle
                .hit_map
                .hit_at(
                    gw_target.x + gw_target.w * 0.5,
                    gw_target.y + gw_target.h * 0.5,
                )
                .expect("hit gateway center");
            assert_eq!(hit.id.as_deref(), Some("gw"));
            assert!(hit.title.contains("Gateway"));
            assert!(hit.lines.iter().any(|l| l.contains("Handles TLS")));

            let edge_target = bundle
                .hit_map
                .targets
                .iter()
                .find(|t| t.id == "edge:gw->auth")
                .expect("edge hit target");
            let edge_hit = bundle
                .hit_map
                .hit_at(
                    edge_target.x + edge_target.w * 0.5,
                    edge_target.y + edge_target.h * 0.5,
                )
                .expect("hit edge midpoint");
            assert_eq!(edge_hit.id.as_deref(), Some("edge:gw->auth"));
        }
    }

    #[test]
    fn handles_empty_graph() {
        let bundle = render_graph_svg(
            &GraphRenderInput {
                nodes: &[],
                edges: &[],
                direction: GraphDirection::Lr,
                selected: None,
                highlighted: &[],
            },
            &Theme::light(),
            400,
            300,
        );
        assert!(bundle.svg.contains("Empty graph"));
        assert!(bundle.hit_map.targets.is_empty());
    }

    #[test]
    fn lr_graph_gives_generous_rank_gaps_and_non_overlapping_cards() {
        let nodes = vec![
            GraphNode {
                id: "gateway".into(),
                label: "Gateway".into(),
                sublabel: Some("envoy 1.31".into()),
                status: Some(Status::Ok),
                group: Some("edge".into()),
                shape: NodeShape::Rounded,
                tooltip: None,
            },
            GraphNode {
                id: "auth".into(),
                label: "Auth".into(),
                sublabel: Some("rust / tonic".into()),
                status: Some(Status::Active),
                group: Some("core".into()),
                shape: NodeShape::Diamond,
                tooltip: None,
            },
            GraphNode {
                id: "cache".into(),
                label: "JWKS Cache".into(),
                sublabel: Some("redis".into()),
                status: Some(Status::Ok),
                group: Some("core".into()),
                shape: NodeShape::Pill,
                tooltip: None,
            },
            GraphNode {
                id: "db".into(),
                label: "Postgres".into(),
                sublabel: Some("primary".into()),
                status: Some(Status::Warn),
                group: Some("data".into()),
                shape: NodeShape::Box,
                tooltip: None,
            },
        ];
        let edges = vec![
            GraphEdge {
                from: "gateway".into(),
                to: "auth".into(),
                label: Some("mTLS gRPC".into()),
                style: EdgeStyle::Solid,
                active: true,
                tooltip: None,
            },
            GraphEdge {
                from: "auth".into(),
                to: "cache".into(),
                label: Some("GET jwks".into()),
                style: EdgeStyle::Dashed,
                active: false,
                tooltip: None,
            },
            GraphEdge {
                from: "auth".into(),
                to: "db".into(),
                label: Some("SELECT revocations".into()),
                style: EdgeStyle::Solid,
                active: true,
                tooltip: None,
            },
        ];

        let bundle = render_graph_svg(
            &GraphRenderInput {
                nodes: &nodes,
                edges: &edges,
                direction: GraphDirection::Lr,
                selected: Some("gateway"),
                highlighted: &[],
            },
            &Theme::light(),
            1200,
            600,
        );

        let targets = &bundle.hit_map.targets;
        assert_eq!(targets.len(), 4);
        for i in 0..targets.len() {
            for j in (i + 1)..targets.len() {
                let a = &targets[i];
                let b = &targets[j];
                let overlaps =
                    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
                assert!(!overlaps, "nodes {} and {} overlap", a.id, b.id);
            }
        }
        let gw = targets.iter().find(|t| t.id == "gateway").unwrap();
        let auth = targets.iter().find(|t| t.id == "auth").unwrap();
        assert!(auth.x - (gw.x + gw.w) > 80.0);
        assert!(auth.w > gw.w);

        let group_lit = render_graph_svg(
            &GraphRenderInput {
                nodes: &nodes,
                edges: &edges,
                direction: GraphDirection::Lr,
                selected: None,
                highlighted: &["core".into()],
            },
            &Theme::dark(),
            1200,
            600,
        );
        assert!(group_lit.svg.contains(r#"<g id="node:auth">"#));
        assert!(group_lit.svg.contains(r#"<g id="node:cache">"#));
        assert!(
            group_lit
                .svg
                .contains(r#"<g id="node:gateway" opacity="0.42">"#)
        );
        assert!(group_lit.svg.contains(r#"<g id="node:db" opacity="0.42">"#));
        assert!(group_lit.svg.contains(r#"<g id="group:core">"#));
        assert!(
            group_lit
                .svg
                .contains(r#"<g id="group:edge" opacity="0.42">"#)
        );
        assert!(group_lit.svg.contains(r#"<g opacity="0.35">"#));

        let edge_lit = render_graph_svg(
            &GraphRenderInput {
                nodes: &nodes,
                edges: &edges,
                direction: GraphDirection::Lr,
                selected: None,
                highlighted: &["gateway->auth".into()],
            },
            &Theme::dark(),
            1200,
            600,
        );
        assert!(edge_lit.svg.contains(r#"<g id="node:gateway">"#));
        assert!(edge_lit.svg.contains(r#"<g id="node:auth">"#));
        assert!(edge_lit.svg.contains(r#"<g id="node:db" opacity="0.42">"#));
    }

    #[test]
    fn narrow_lr_chain_auto_flips_to_tb_for_larger_cards() {
        let nodes = vec![
            GraphNode {
                id: "n1".into(),
                label: "Ingest".into(),
                sublabel: Some("http".into()),
                status: Some(Status::Ok),
                group: Some("edge".into()),
                shape: NodeShape::Rounded,
                tooltip: None,
            },
            GraphNode {
                id: "n2".into(),
                label: "Validate".into(),
                sublabel: Some("schema".into()),
                status: Some(Status::Ok),
                group: Some("core".into()),
                shape: NodeShape::Rounded,
                tooltip: None,
            },
            GraphNode {
                id: "n3".into(),
                label: "Transform".into(),
                sublabel: Some("worker".into()),
                status: Some(Status::Active),
                group: Some("core".into()),
                shape: NodeShape::Rounded,
                tooltip: None,
            },
            GraphNode {
                id: "n4".into(),
                label: "Persist".into(),
                sublabel: Some("postgres".into()),
                status: Some(Status::Ok),
                group: Some("data".into()),
                shape: NodeShape::Rounded,
                tooltip: None,
            },
        ];
        let edges = vec![
            GraphEdge {
                from: "n1".into(),
                to: "n2".into(),
                label: Some("mTLS gRPC".into()),
                style: EdgeStyle::Solid,
                active: true,
                tooltip: None,
            },
            GraphEdge {
                from: "n2".into(),
                to: "n3".into(),
                label: Some("queue".into()),
                style: EdgeStyle::Solid,
                active: false,
                tooltip: None,
            },
            GraphEdge {
                from: "n3".into(),
                to: "n4".into(),
                label: Some("batch write".into()),
                style: EdgeStyle::Solid,
                active: false,
                tooltip: None,
            },
        ];

        let bundle = render_graph_svg(
            &GraphRenderInput {
                nodes: &nodes,
                edges: &edges,
                direction: GraphDirection::Lr,
                selected: None,
                highlighted: &[],
            },
            &Theme::light(),
            460,
            360,
        );

        let n1 = bundle
            .hit_map
            .targets
            .iter()
            .find(|t| t.id == "n1")
            .unwrap();
        let n2 = bundle
            .hit_map
            .targets
            .iter()
            .find(|t| t.id == "n2")
            .unwrap();
        let n3 = bundle
            .hit_map
            .targets
            .iter()
            .find(|t| t.id == "n3")
            .unwrap();
        let n4 = bundle
            .hit_map
            .targets
            .iter()
            .find(|t| t.id == "n4")
            .unwrap();

        assert!(n2.y > n1.y && n3.y > n2.y && n4.y > n3.y);
        assert!((n1.x - n4.x).abs() < 1.0);

        let mut graph: StableDiGraph<usize, usize> = StableDiGraph::new();
        let mut id_to_idx = HashMap::new();
        let mut id_to_pos = HashMap::new();
        let mut card_sizes = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            id_to_idx.insert(node.id.as_str(), graph.add_node(i));
            id_to_pos.insert(node.id.as_str(), i);
            card_sizes.push(measure_card(node));
        }
        for (i, edge) in edges.iter().enumerate() {
            graph.add_edge(
                id_to_idx[edge.from.as_str()],
                id_to_idx[edge.to.as_str()],
                i,
            );
        }
        let lr = compute_layout(
            &graph,
            &nodes,
            &edges,
            &id_to_pos,
            &card_sizes,
            GraphDirection::Lr,
        );
        let tb = compute_layout(
            &graph,
            &nodes,
            &edges,
            &id_to_pos,
            &card_sizes,
            GraphDirection::Tb,
        );
        let avail_w = 460.0 - 48.0;
        let avail_h = 360.0 - 48.0;
        let lr_scale = (avail_w / lr.span_w).min(avail_h / lr.span_h).min(2.0);
        let tb_scale = (avail_w / tb.span_w).min(avail_h / tb.span_h).min(2.0);
        assert!(tb_scale > lr_scale * 1.25);
        assert!(n1.w > (card_sizes[0].0 as f32) * lr_scale * 1.25);
    }

    #[test]
    fn pill_with_long_sublabel_adds_endcap_clearance() {
        let sub = "us-east-1.prod.internal:8443";
        assert_eq!(sub.chars().count(), 28);
        let pill = GraphNode {
            id: "svc".into(),
            label: "Edge Proxy".into(),
            sublabel: Some(sub.into()),
            status: Some(Status::Ok),
            group: None,
            shape: NodeShape::Pill,
            tooltip: None,
        };
        let rounded = GraphNode {
            shape: NodeShape::Rounded,
            ..pill.clone()
        };

        let (pill_w, pill_h) = measure_card(&pill);
        let (rounded_w, _) = measure_card(&rounded);
        let sub_text_w = (sub.chars().count() as f64) * 6.8;
        assert!(pill_w > rounded_w);
        assert!(pill_w >= sub_text_w + pill_h + 24.0);
    }
}
