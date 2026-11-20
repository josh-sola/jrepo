use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::svg::{FitTransform, HitMap, HitTarget, SvgBundle, compute_fit_transform, usvg_options};
use crate::theme::Theme;

struct RawBox {
    id: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

pub fn prepare_custom_svg(
    source: &str,
    tooltips: &BTreeMap<String, String>,
    selected: Option<&str>,
    highlighted: &[String],
    theme: &Theme,
    target_w: u32,
    target_h: u32,
) -> Result<SvgBundle, String> {
    let resolved = theme.resolve_svg_vars(source);
    let xml_titles = extract_xml_titles(&resolved)?;

    let tree = usvg::Tree::from_str(&resolved, &usvg_options())
        .map_err(|e| format!("failed to parse SVG with usvg: {e}"))?;
    let size = tree.size();
    let fit = compute_fit_transform(size.width(), size.height(), target_w, target_h);

    let mut raw_boxes = Vec::new();
    collect_usvg_boxes(tree.root(), &mut raw_boxes);

    let mut targets = Vec::with_capacity(raw_boxes.len());
    let mut overlay_rects = String::new();

    for b in &raw_boxes {
        let tooltip = tooltips
            .get(&b.id)
            .or_else(|| xml_titles.get(&b.id))
            .cloned()
            .unwrap_or_else(|| b.id.clone());

        targets.push(HitTarget {
            id: b.id.clone(),
            x: b.x * fit.scale + fit.offset_x,
            y: b.y * fit.scale + fit.offset_y,
            w: b.w * fit.scale,
            h: b.h * fit.scale,
            tooltip,
        });

        let is_selected = selected == Some(b.id.as_str());
        let is_highlighted = highlighted.iter().any(|h| h == &b.id);
        if is_selected || is_highlighted {
            let stroke = if is_selected {
                theme.accent.hex
            } else {
                theme.highlight.hex
            };
            let stroke_w = if is_selected { 2.5 } else { 2.0 } / fit.scale.max(0.25);
            let pad = 2.0 / fit.scale.max(0.25);
            let _ = writeln!(
                overlay_rects,
                r#"  <rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{:.2}" fill="none" stroke="{stroke}" stroke-width="{stroke_w:.2}"/>"#,
                b.x - pad,
                b.y - pad,
                b.w + pad * 2.0,
                b.h + pad * 2.0,
                4.0 / fit.scale.max(0.25),
            );
        }
    }

    let final_svg = if overlay_rects.is_empty() {
        resolved
    } else {
        inject_overlay(&resolved, &overlay_rects)
    };

    let tw = target_w.max(1);
    let th = target_h.max(1);

    Ok(SvgBundle {
        svg: final_svg,
        width: tw,
        height: th,
        hit_map: HitMap {
            targets,
            chart_hover: None,
        },
    })
}

pub fn custom_svg_fit_transform(
    svg: &str,
    target_w: u32,
    target_h: u32,
) -> Result<FitTransform, String> {
    let tree = usvg::Tree::from_str(svg, &usvg_options())
        .map_err(|e| format!("failed to parse SVG with usvg: {e}"))?;
    let size = tree.size();
    Ok(compute_fit_transform(
        size.width(),
        size.height(),
        target_w,
        target_h,
    ))
}

/// Wraps a custom SVG in a `target_w`x`target_h` root that scales it the way
/// `compute_fit_transform` does (centered, aspect kept), so the hit map from
/// `prepare_custom_svg` lines up with the SVG as a browser draws it.
pub fn fit_custom_svg(svg: &str, target_w: u32, target_h: u32) -> Result<String, String> {
    let tree = usvg::Tree::from_str(svg, &usvg_options())
        .map_err(|e| format!("failed to parse SVG with usvg: {e}"))?;
    let size = tree.size();
    let doc = roxmltree::Document::parse(svg).map_err(|e| format!("invalid SVG XML: {e}"))?;
    let inner = &svg[doc.root_element().range()];
    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{tw}" height="{th}" viewBox="0 0 {sw} {sh}">{inner}</svg>"#,
        tw = target_w.max(1),
        th = target_h.max(1),
        sw = size.width(),
        sh = size.height(),
    ))
}

fn extract_xml_titles(svg: &str) -> Result<BTreeMap<String, String>, String> {
    let doc = roxmltree::Document::parse(svg).map_err(|e| format!("invalid SVG XML: {e}"))?;
    let mut titles = BTreeMap::new();
    for node in doc.descendants() {
        if !node.is_element() {
            continue;
        }
        let Some(id) = node.attribute("id").filter(|s| !s.is_empty()) else {
            continue;
        };
        if let Some(title_el) = node
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "title")
        {
            let text: String = title_el
                .descendants()
                .filter(|d| d.is_text())
                .filter_map(|d| d.text())
                .collect::<String>()
                .trim()
                .to_string();
            if !text.is_empty() {
                titles.insert(id.to_string(), text);
            }
        }
    }
    Ok(titles)
}

fn collect_usvg_boxes(group: &usvg::Group, out: &mut Vec<RawBox>) {
    for node in group.children() {
        let id = node.id();
        if !id.is_empty()
            && let Some(bbox) = node.abs_bounding_box()
            && bbox.width() > 0.0
            && bbox.height() > 0.0
        {
            out.push(RawBox {
                id: id.to_string(),
                x: bbox.x(),
                y: bbox.y(),
                w: bbox.width(),
                h: bbox.height(),
            });
        }
        if let usvg::Node::Group(sub) = node {
            collect_usvg_boxes(sub, out);
        }
    }
}

fn inject_overlay(svg: &str, overlay_rects: &str) -> String {
    let group = format!("\n<g id=\"__viz_overlay\">\n{overlay_rects}</g>\n");
    if let Some(pos) = svg.rfind("</svg>") {
        let mut out = String::with_capacity(svg.len() + group.len());
        out.push_str(&svg[..pos]);
        out.push_str(&group);
        out.push_str(&svg[pos..]);
        out
    } else {
        let mut out = svg.to_string();
        out.push_str(&group);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_vars_extracts_titles_and_hit_tests_usvg_boxes() {
        let src = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100">
            <rect width="200" height="100" fill="var(--bg)"/>
            <g id="ingress">
                <title>From SVG title</title>
                <rect x="10" y="20" width="60" height="40" fill="var(--surface)" stroke="var(--border)"/>
            </g>
            <rect id="db" x="120" y="20" width="60" height="40" fill="var(--accent)">
                <title>Original DB title</title>
            </rect>
        </svg>"#;

        let mut tooltips = BTreeMap::new();
        tooltips.insert("db".into(), "Overridden DB tooltip".into());

        let bundle = prepare_custom_svg(
            src,
            &tooltips,
            Some("ingress"),
            &["db".into()],
            &Theme::dark(),
            400,
            200,
        )
        .expect("prepare custom SVG");

        assert!(!bundle.svg.contains("var(--"));
        assert!(bundle.svg.contains(r#"id="__viz_overlay""#));

        let hit_in = bundle.hit_map.hit_at(80.0, 80.0).expect("hit ingress");
        assert_eq!(hit_in.id.as_deref(), Some("ingress"));
        assert_eq!(hit_in.title, "From SVG title");

        let hit_db = bundle.hit_map.hit_at(300.0, 80.0).expect("hit db");
        assert_eq!(hit_db.id.as_deref(), Some("db"));
        assert_eq!(hit_db.title, "Overridden DB tooltip");
    }
}
