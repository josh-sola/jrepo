use std::collections::VecDeque;
use std::sync::Arc;

use image::RgbaImage;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

use crate::svg::{FitTransform, HitMap, SvgBundle, compute_fit_transform, usvg_options};
use crate::theme::ThemeMode;

#[derive(Debug, Clone)]
pub struct RasterizedGraphic {
    pub image: RgbaImage,
    pub hit_map: HitMap,
    pub transform: FitTransform,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RasterCacheKey {
    pub pane_id: String,
    pub theme_mode: ThemeMode,
    pub content_hash: u64,
    pub px_w: u32,
    pub px_h: u32,
}

pub struct RasterCache {
    max_entries: usize,
    entries: VecDeque<(RasterCacheKey, Arc<RasterizedGraphic>)>,
}

impl Default for RasterCache {
    fn default() -> Self {
        Self::new(32)
    }
}

impl RasterCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            entries: VecDeque::with_capacity(max_entries.max(1)),
        }
    }

    pub fn get(&mut self, key: &RasterCacheKey) -> Option<Arc<RasterizedGraphic>> {
        let pos = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(pos)?;
        let value = Arc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(value)
    }

    pub fn insert(&mut self, key: RasterCacheKey, value: Arc<RasterizedGraphic>) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k == &key) {
            self.entries.remove(pos);
        }
        while self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back((key, value));
    }

    pub fn get_or_render(
        &mut self,
        key: RasterCacheKey,
        render_fn: impl FnOnce() -> Result<SvgBundle, String>,
    ) -> Result<Arc<RasterizedGraphic>, String> {
        if let Some(hit) = self.get(&key) {
            return Ok(hit);
        }
        let bundle = render_fn()?;
        let (image, transform) = rasterize_svg(&bundle.svg, key.px_w, key.px_h)?;
        let hit_map = if bundle.width == key.px_w && bundle.height == key.px_h {
            bundle.hit_map
        } else {
            let remap = compute_fit_transform(
                bundle.width as f32,
                bundle.height as f32,
                key.px_w,
                key.px_h,
            );
            bundle.hit_map.transformed(remap)
        };
        let graphic = Arc::new(RasterizedGraphic {
            image,
            hit_map,
            transform,
        });
        self.insert(key, Arc::clone(&graphic));
        Ok(graphic)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub fn rasterize_svg(svg: &str, px_w: u32, px_h: u32) -> Result<(RgbaImage, FitTransform), String> {
    let w = px_w.max(1);
    let h = px_h.max(1);
    let tree = usvg::Tree::from_str(svg, &usvg_options())
        .map_err(|e| format!("failed to parse SVG for rasterization: {e}"))?;
    let size = tree.size();
    let fit = compute_fit_transform(size.width(), size.height(), w, h);

    let mut pixmap =
        Pixmap::new(w, h).ok_or_else(|| format!("failed to allocate {w}x{h} pixmap"))?;
    let ts = Transform::from_translate(fit.offset_x, fit.offset_y).pre_scale(fit.scale, fit.scale);
    resvg::render(&tree, ts, &mut pixmap.as_mut());

    // tiny_skia renders premultiplied RGBA; un-premultiply so image::RgbaImage
    // encodes straight alpha without dark fringes on anti-aliased edges.
    let mut straight = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in pixmap.pixels() {
        let a = px.alpha();
        if a == 0 {
            straight.extend_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            straight.extend_from_slice(&[px.red(), px.green(), px.blue(), 255]);
        } else {
            let af = f32::from(a);
            let r = ((f32::from(px.red()) * 255.0) / af).round().min(255.0) as u8;
            let g = ((f32::from(px.green()) * 255.0) / af).round().min(255.0) as u8;
            let b = ((f32::from(px.blue()) * 255.0) / af).round().min(255.0) as u8;
            straight.extend_from_slice(&[r, g, b, a]);
        }
    }

    let image = RgbaImage::from_raw(w, h, straight)
        .ok_or_else(|| "pixel buffer length mismatch".to_string())?;
    Ok((image, fit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ChartPoint, ChartSeries, ChartType, EdgeStyle, GraphDirection, GraphEdge, GraphNode,
        NodeShape, Status,
    };
    use crate::svg::chart::{ChartRenderInput, render_chart_svg};
    use crate::svg::graph::{GraphRenderInput, render_graph_svg};
    use crate::svg::hit_test::prepare_custom_svg;
    use crate::theme::Theme;
    use std::collections::BTreeMap;

    #[test]
    fn rasterizes_custom_svg_and_caches_with_eviction() {
        let src = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 100 50">
            <rect id="box" x="10" y="10" width="80" height="30" fill="var(--accent)"/>
            <text x="50" y="30" text-anchor="middle" font-family="Inter, sans-serif" font-size="12" fill="var(--bg)">Hi</text>
        </svg>"#;

        let bundle = prepare_custom_svg(src, &BTreeMap::new(), None, &[], &Theme::dark(), 200, 100)
            .expect("prepare svg");
        let (img, fit) = rasterize_svg(&bundle.svg, 200, 100).expect("rasterize");
        assert_eq!(img.dimensions(), (200, 100));
        assert!((fit.scale - 2.0).abs() < 1e-4);
        assert!(img.pixels().any(|p| p.0[3] > 0 && p.0[2] > 100));

        let mut cache = RasterCache::new(2);
        let k1 = RasterCacheKey {
            pane_id: "p1".into(),
            theme_mode: ThemeMode::Dark,
            content_hash: 1,
            px_w: 200,
            px_h: 100,
        };
        let k2 = RasterCacheKey {
            content_hash: 2,
            ..k1.clone()
        };
        let k3 = RasterCacheKey {
            content_hash: 3,
            ..k1.clone()
        };

        let mut render_calls = 0usize;
        let _ = cache
            .get_or_render(k1.clone(), || {
                render_calls += 1;
                Ok(bundle.clone())
            })
            .unwrap();
        let _ = cache
            .get_or_render(k1.clone(), || {
                render_calls += 1;
                Ok(bundle.clone())
            })
            .unwrap();
        assert_eq!(render_calls, 1);

        let _ = cache.get_or_render(k2, || Ok(bundle.clone())).unwrap();
        let _ = cache.get_or_render(k3, || Ok(bundle.clone())).unwrap();
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&k1).is_none());
    }

    #[test]
    fn rasterizes_graph_and_chart_svgs_with_bundled_fonts() {
        let nodes = vec![
            GraphNode {
                id: "a".into(),
                label: "Ingress".into(),
                sublabel: Some("envoy".into()),
                status: Some(Status::Ok),
                group: Some("edge".into()),
                shape: NodeShape::Rounded,
                tooltip: None,
            },
            GraphNode {
                id: "b".into(),
                label: "Backend".into(),
                sublabel: None,
                status: Some(Status::Active),
                group: None,
                shape: NodeShape::Box,
                tooltip: None,
            },
        ];
        let edges = vec![GraphEdge {
            from: "a".into(),
            to: "b".into(),
            label: Some("HTTP".into()),
            style: EdgeStyle::Solid,
            active: true,
            tooltip: None,
        }];
        let graph_bundle = render_graph_svg(
            &GraphRenderInput {
                nodes: &nodes,
                edges: &edges,
                direction: GraphDirection::Lr,
                selected: Some("a"),
                highlighted: &[],
            },
            &Theme::dark(),
            480,
            240,
        );
        let (graph_img, _) = rasterize_svg(&graph_bundle.svg, 480, 240).expect("rasterize graph");
        assert!(graph_img.pixels().any(|p| p.0[3] > 0));

        let series = vec![ChartSeries {
            name: "rps".into(),
            color: None,
            data: vec![
                ChartPoint { x: None, y: 10.0 },
                ChartPoint { x: None, y: 25.0 },
            ],
        }];
        let labels = vec!["t0".into(), "t1".into()];
        let chart_bundle = render_chart_svg(
            &ChartRenderInput {
                id: "c",
                title: Some("Throughput"),
                chart_type: ChartType::Area,
                x_labels: &labels,
                x_unit: None,
                y_unit: Some("req/s"),
                value_labels: false,
                max_override: None,
                series: &series,
            },
            &Theme::light(),
            480,
            240,
        )
        .expect("render chart");
        let (chart_img, _) = rasterize_svg(&chart_bundle.svg, 480, 240).expect("rasterize chart");
        assert!(chart_img.pixels().any(|p| p.0[3] > 0));

        for theme in [Theme::light(), Theme::dark()] {
            let hbar_bundle = render_chart_svg(
                &ChartRenderInput {
                    id: "hb",
                    title: Some("Build Time"),
                    chart_type: ChartType::HBar,
                    x_labels: &labels,
                    x_unit: Some("seconds"),
                    y_unit: None,
                    value_labels: true,
                    max_override: None,
                    series: &series,
                },
                &theme,
                520,
                260,
            )
            .expect("render hbar");
            let (hbar_img, _) = rasterize_svg(&hbar_bundle.svg, 520, 260).expect("rasterize hbar");
            assert!(hbar_img.pixels().any(|p| p.0[3] > 0));
        }
    }
}
