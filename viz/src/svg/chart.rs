use std::sync::OnceLock;

use plotters::prelude::*;
use plotters::style::register_font;
use plotters::style::text_anchor::{HPos, Pos, VPos};

use crate::model::{ChartSeries, ChartType};
use crate::svg::{
    ChartBucket, ChartHoverIndex, HitMap, HorizontalChartBucket, INTER_TTF,
    JETBRAINS_MONO_BOLD_TTF, ScatterPoint, SvgBundle,
};
use crate::theme::{Theme, ThemeColor, parse_hex_rgb};

static PLOTTERS_FONTS: OnceLock<()> = OnceLock::new();

fn ensure_plotters_fonts() {
    PLOTTERS_FONTS.get_or_init(|| {
        let _ = register_font("sans-serif", FontStyle::Normal, INTER_TTF);
        let _ = register_font("sans-serif", FontStyle::Bold, JETBRAINS_MONO_BOLD_TTF);
        let _ = register_font("Inter", FontStyle::Normal, INTER_TTF);
    });
}

fn to_rgb(color: ThemeColor) -> RGBColor {
    RGBColor(color.r(), color.g(), color.b())
}

fn series_color(series: &ChartSeries, idx: usize, theme: &Theme) -> RGBColor {
    if let Some(hex) = series.color.as_deref()
        && let Some((r, g, b)) = parse_hex_rgb(hex)
    {
        return RGBColor(r, g, b);
    }
    to_rgb(theme.chart_palette[idx % theme.chart_palette.len()])
}

pub struct ChartRenderInput<'a> {
    pub id: &'a str,
    pub title: Option<&'a str>,
    pub chart_type: ChartType,
    pub x_labels: &'a [String],
    pub x_unit: Option<&'a str>,
    pub y_unit: Option<&'a str>,
    pub value_labels: bool,
    pub max_override: Option<f64>,
    pub series: &'a [ChartSeries],
}

pub fn render_chart_svg(
    input: &ChartRenderInput<'_>,
    theme: &Theme,
    width: u32,
    height: u32,
) -> Result<SvgBundle, String> {
    ensure_plotters_fonts();

    let w = width.max(120);
    let h = height.max(90);

    let has_points = input.series.iter().any(|s| !s.data.is_empty());
    if !has_points {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" fill="{bg}"/><text x="{cx}" y="{cy}" text-anchor="middle" dominant-baseline="middle" font-family="Inter, sans-serif" font-size="14" fill="{muted}">No chart data</text></svg>"##,
            bg = theme.bg.hex,
            cx = w / 2,
            cy = h / 2,
            muted = theme.muted.hex,
        );
        return Ok(SvgBundle {
            svg,
            width: w,
            height: h,
            hit_map: HitMap::default(),
        });
    }

    if input.chart_type == ChartType::HBar {
        return render_hbar_svg(input, theme, w, h);
    }

    let id = input.id;
    let title = input.title;
    let chart_type = input.chart_type;
    let x_labels = input.x_labels;
    let y_unit = input.y_unit;
    let series = input.series;

    let (x_min, x_max, y_min, y_max, max_len) =
        compute_bounds(chart_type, series, input.max_override);

    let mut svg_buf = String::with_capacity(4096);
    let hover_index = {
        let root = SVGBackend::with_string(&mut svg_buf, (w, h)).into_drawing_area();
        root.fill(&to_rgb(theme.bg))
            .map_err(|e| format!("plotters fill failed: {e}"))?;

        let mut builder = ChartBuilder::on(&root);
        builder
            .margin(14)
            .x_label_area_size(34)
            .y_label_area_size(52);
        if let Some(t) = title {
            builder.caption(t, ("sans-serif", 16).into_font().color(&to_rgb(theme.fg)));
        }

        let mut chart = builder
            .build_cartesian_2d(x_min..x_max, y_min..y_max)
            .map_err(|e| format!("plotters chart build failed: {e}"))?;

        let unit_suffix = y_unit.unwrap_or("");
        chart
            .configure_mesh()
            .axis_style(to_rgb(theme.border))
            .bold_line_style(to_rgb(theme.border).mix(0.45))
            .light_line_style(to_rgb(theme.border).mix(0.18))
            .label_style(("sans-serif", 11).into_font().color(&to_rgb(theme.muted)))
            .x_label_formatter(&|x| {
                if chart_type == ChartType::Scatter {
                    format!("{x:.1}")
                } else {
                    let idx = x.round() as isize;
                    if idx >= 0 {
                        x_labels
                            .get(idx as usize)
                            .cloned()
                            .unwrap_or_else(|| idx.to_string())
                    } else {
                        String::new()
                    }
                }
            })
            .y_label_formatter(&|y| {
                if unit_suffix.is_empty() {
                    fmt_axis(*y)
                } else {
                    format!("{}{unit_suffix}", fmt_axis(*y))
                }
            })
            .draw()
            .map_err(|e| format!("plotters mesh draw failed: {e}"))?;

        match chart_type {
            ChartType::Line => {
                for (s_idx, s) in series.iter().enumerate() {
                    let color = series_color(s, s_idx, theme);
                    let pts: Vec<(f64, f64)> = s
                        .data
                        .iter()
                        .enumerate()
                        .map(|(i, p)| (p.x.unwrap_or(i as f64), p.y))
                        .collect();
                    chart
                        .draw_series(LineSeries::new(pts.iter().copied(), color.stroke_width(2)))
                        .map_err(|e| format!("plotters line draw failed: {e}"))?
                        .label(s.name.clone())
                        .legend(move |(x, y)| {
                            PathElement::new(vec![(x, y), (x + 14, y)], color.stroke_width(2))
                        });
                    chart
                        .draw_series(
                            pts.iter()
                                .map(|&(x, y)| Circle::new((x, y), 3, color.filled())),
                        )
                        .map_err(|e| format!("plotters marker draw failed: {e}"))?;
                }
            }
            ChartType::Area => {
                for (s_idx, s) in series.iter().enumerate() {
                    let color = series_color(s, s_idx, theme);
                    let pts: Vec<(f64, f64)> = s
                        .data
                        .iter()
                        .enumerate()
                        .map(|(i, p)| (p.x.unwrap_or(i as f64), p.y))
                        .collect();
                    let baseline = y_min.max(0.0).min(y_max);
                    chart
                        .draw_series(
                            AreaSeries::new(pts.iter().copied(), baseline, color.mix(0.24))
                                .border_style(color.stroke_width(2)),
                        )
                        .map_err(|e| format!("plotters area draw failed: {e}"))?
                        .label(s.name.clone())
                        .legend(move |(x, y)| {
                            Rectangle::new([(x, y - 5), (x + 14, y + 5)], color.filled())
                        });
                }
            }
            ChartType::Bar => {
                let n_series = series.len().max(1) as f64;
                let group_width = 0.72f64;
                let bar_w = group_width / n_series;
                let baseline = y_min.max(0.0).min(y_max);
                for (s_idx, s) in series.iter().enumerate() {
                    let color = series_color(s, s_idx, theme);
                    let offset = -group_width * 0.5 + (s_idx as f64) * bar_w;
                    chart
                        .draw_series(s.data.iter().enumerate().map(|(i, p)| {
                            let cx = p.x.unwrap_or(i as f64);
                            let x0 = cx + offset + bar_w * 0.08;
                            let x1 = cx + offset + bar_w * 0.92;
                            let (lo, hi) = if p.y >= baseline {
                                (baseline, p.y)
                            } else {
                                (p.y, baseline)
                            };
                            Rectangle::new([(x0, hi), (x1, lo)], color.filled())
                        }))
                        .map_err(|e| format!("plotters bar draw failed: {e}"))?
                        .label(s.name.clone())
                        .legend(move |(x, y)| {
                            Rectangle::new([(x, y - 5), (x + 14, y + 5)], color.filled())
                        });
                }
            }
            ChartType::Scatter => {
                for (s_idx, s) in series.iter().enumerate() {
                    let color = series_color(s, s_idx, theme);
                    chart
                        .draw_series(s.data.iter().enumerate().map(|(i, p)| {
                            let x = p.x.unwrap_or(i as f64);
                            Circle::new((x, p.y), 4, color.filled())
                        }))
                        .map_err(|e| format!("plotters scatter draw failed: {e}"))?
                        .label(s.name.clone())
                        .legend(move |(x, y)| Circle::new((x + 7, y), 4, color.filled()));
                }
            }
            ChartType::HBar => unreachable!("handled above"),
        }

        if series.len() > 1 || series.first().is_some_and(|s| !s.name.is_empty()) {
            chart
                .configure_series_labels()
                .position(SeriesLabelPosition::UpperRight)
                .background_style(to_rgb(theme.surface).mix(0.9))
                .border_style(to_rgb(theme.border))
                .label_font(("sans-serif", 11).into_font().color(&to_rgb(theme.fg)))
                .draw()
                .map_err(|e| format!("plotters legend draw failed: {e}"))?;
        }

        let (x_px_range, y_px_range) = chart.plotting_area().get_pixel_range();
        let plot_x = (x_px_range.start as f32, x_px_range.end as f32);
        let plot_y = (y_px_range.start as f32, y_px_range.end as f32);

        let hover = match chart_type {
            ChartType::Scatter => {
                let mut points = Vec::new();
                for (s_idx, s) in series.iter().enumerate() {
                    for (p_idx, pt) in s.data.iter().enumerate() {
                        let x = pt.x.unwrap_or(p_idx as f64);
                        let (px, py) = chart.backend_coord(&(x, pt.y));
                        points.push(ScatterPoint {
                            id: format!("{id}:{s_idx}:{p_idx}"),
                            px: px as f32,
                            py: py as f32,
                            series: s.name.clone(),
                            x,
                            y: pt.y,
                        });
                    }
                }
                ChartHoverIndex::Scatter {
                    plot_x,
                    plot_y,
                    points,
                    y_unit: y_unit.map(ToString::to_string),
                }
            }
            ChartType::Line | ChartType::Area | ChartType::Bar | ChartType::HBar => {
                let x_coords: Vec<f32> = (0..max_len)
                    .map(|i| {
                        let x_val = series
                            .iter()
                            .find_map(|s| s.data.get(i).and_then(|p| p.x))
                            .unwrap_or(i as f64);
                        chart.backend_coord(&(x_val, y_min)).0 as f32
                    })
                    .collect();

                let mut buckets = Vec::with_capacity(max_len);
                for (i, &x_px) in x_coords.iter().enumerate() {
                    let x_min_px = if i == 0 {
                        plot_x.0
                    } else {
                        (x_coords[i - 1] + x_px) * 0.5
                    };
                    let x_max_px = if i + 1 == x_coords.len() {
                        plot_x.1
                    } else {
                        (x_px + x_coords[i + 1]) * 0.5
                    };
                    let x_label = x_labels.get(i).cloned().unwrap_or_else(|| format!("#{i}"));
                    let values = series
                        .iter()
                        .filter_map(|s| s.data.get(i).map(|pt| (s.name.clone(), pt.y)))
                        .collect();
                    buckets.push(ChartBucket {
                        id: format!("{id}:{x_label}"),
                        x_px,
                        x_min: x_min_px,
                        x_max: x_max_px,
                        x_label,
                        values,
                    });
                }

                ChartHoverIndex::Buckets {
                    plot_x,
                    plot_y,
                    buckets,
                    y_unit: y_unit.map(ToString::to_string),
                }
            }
        };

        root.present()
            .map_err(|e| format!("plotters present failed: {e}"))?;
        hover
    };

    Ok(SvgBundle {
        svg: svg_buf,
        width: w,
        height: h,
        hit_map: HitMap {
            targets: Vec::new(),
            chart_hover: Some(hover_index),
        },
    })
}

fn render_hbar_svg(
    input: &ChartRenderInput<'_>,
    theme: &Theme,
    w: u32,
    h: u32,
) -> Result<SvgBundle, String> {
    let series = input.series;
    let x_labels = input.x_labels;
    let unit = input.x_unit.or(input.y_unit);

    let max_len = series
        .iter()
        .map(|s| s.data.len())
        .max()
        .unwrap_or(0)
        .max(1);
    let mut v_min = 0.0f64;
    let mut v_max = 0.0f64;
    for s in series {
        for pt in &s.data {
            v_min = v_min.min(pt.y);
            v_max = v_max.max(pt.y);
        }
    }
    if let Some(m) = input.max_override {
        v_max = v_max.max(m);
    }
    let span = (v_max - v_min).abs().max(1.0);
    let right_pad = if input.value_labels {
        span * 0.20
    } else {
        span * 0.08
    };
    let x_min = if v_min < 0.0 {
        v_min - span * 0.08
    } else {
        0.0
    };
    let x_max = v_max + right_pad;

    let y_min = -0.5f64;
    let y_max = (max_len as f64) - 0.5;

    let longest_cat = x_labels
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(4);
    let y_label_w = ((longest_cat * 7 + 20) as u32).clamp(64, 170);
    let x_label_h = if unit.is_some() { 40 } else { 30 };

    let mut svg_buf = String::with_capacity(4096);
    let hover_index = {
        let root = SVGBackend::with_string(&mut svg_buf, (w, h)).into_drawing_area();
        root.fill(&to_rgb(theme.bg))
            .map_err(|e| format!("plotters fill failed: {e}"))?;

        let mut builder = ChartBuilder::on(&root);
        builder
            .margin(14)
            .margin_right(20)
            .x_label_area_size(x_label_h)
            .y_label_area_size(y_label_w);
        if let Some(t) = input.title {
            builder.caption(t, ("sans-serif", 16).into_font().color(&to_rgb(theme.fg)));
        }

        let mut chart = builder
            .build_cartesian_2d(x_min..x_max, y_min..y_max)
            .map_err(|e| format!("plotters hbar build failed: {e}"))?;

        let y_fmt = |y: &f64| {
            // Reverse category order so index 0 appears at the top of the chart.
            if (y - y.round()).abs() > 0.05 {
                return String::new();
            }
            let yi = y.round() as isize;
            if yi < 0 || yi >= max_len as isize {
                return String::new();
            }
            let cat_idx = max_len - 1 - (yi as usize);
            x_labels
                .get(cat_idx)
                .cloned()
                .unwrap_or_else(|| format!("#{cat_idx}"))
        };
        let x_fmt = |x: &f64| fmt_axis(*x);

        let mut mesh = chart.configure_mesh();
        mesh.axis_style(to_rgb(theme.border))
            .bold_line_style(to_rgb(theme.border).mix(0.45))
            .light_line_style(to_rgb(theme.border).mix(0.18))
            .disable_y_mesh()
            .y_labels(max_len * 2 + 1)
            .label_style(("sans-serif", 11).into_font().color(&to_rgb(theme.muted)))
            .axis_desc_style(("sans-serif", 11).into_font().color(&to_rgb(theme.muted)))
            .y_label_formatter(&y_fmt)
            .x_label_formatter(&x_fmt);
        if let Some(u) = unit {
            mesh.x_desc(u);
        }
        mesh.draw()
            .map_err(|e| format!("plotters hbar mesh failed: {e}"))?;

        let n_series = series.len().max(1) as f64;
        let group_h = 0.72f64;
        let bar_h = group_h / n_series;
        let label_offset = (x_max - x_min) * 0.015;

        for (s_idx, s) in series.iter().enumerate() {
            let color = series_color(s, s_idx, theme);
            chart
                .draw_series(s.data.iter().enumerate().map(|(i, pt)| {
                    let cat_y = (max_len - 1 - i.min(max_len - 1)) as f64;
                    let slot_top = cat_y + group_h * 0.5 - (s_idx as f64) * bar_h;
                    let y1 = slot_top - bar_h * 0.08;
                    let y0 = slot_top - bar_h * 0.92;
                    let (lo, hi) = if pt.y >= 0.0 {
                        (0.0, pt.y)
                    } else {
                        (pt.y, 0.0)
                    };
                    Rectangle::new([(lo, y1), (hi, y0)], color.filled())
                }))
                .map_err(|e| format!("plotters hbar draw failed: {e}"))?
                .label(s.name.clone())
                .legend(move |(x, y)| {
                    Rectangle::new([(x, y - 5), (x + 14, y + 5)], color.filled())
                });

            if input.value_labels {
                let label_style = ("sans-serif", 11)
                    .into_font()
                    .color(&to_rgb(theme.fg))
                    .pos(Pos::new(HPos::Left, VPos::Center));
                chart
                    .draw_series(s.data.iter().enumerate().map(|(i, pt)| {
                        let cat_y = (max_len - 1 - i.min(max_len - 1)) as f64;
                        let slot_top = cat_y + group_h * 0.5 - (s_idx as f64) * bar_h;
                        let mid_y = slot_top - bar_h * 0.5;
                        let tx = pt.y.max(0.0) + label_offset;
                        Text::new(fmt_value_label(pt.y), (tx, mid_y), label_style.clone())
                    }))
                    .map_err(|e| format!("plotters hbar label failed: {e}"))?;
            }
        }

        if series.len() > 1 || series.first().is_some_and(|s| !s.name.is_empty()) {
            chart
                .configure_series_labels()
                .position(SeriesLabelPosition::LowerRight)
                .background_style(to_rgb(theme.surface).mix(0.92))
                .border_style(to_rgb(theme.border))
                .label_font(("sans-serif", 11).into_font().color(&to_rgb(theme.fg)))
                .draw()
                .map_err(|e| format!("plotters hbar legend failed: {e}"))?;
        }

        let (x_px_range, y_px_range) = chart.plotting_area().get_pixel_range();
        let plot_x = (x_px_range.start as f32, x_px_range.end as f32);
        let plot_y = (y_px_range.start as f32, y_px_range.end as f32);

        let y_coords: Vec<f32> = (0..max_len)
            .map(|i| {
                let cat_y = (max_len - 1 - i) as f64;
                chart.backend_coord(&(0.0, cat_y)).1 as f32
            })
            .collect();

        let mut buckets = Vec::with_capacity(max_len);
        for (i, &y_px) in y_coords.iter().enumerate() {
            let y_min_px = if i == 0 {
                plot_y.0
            } else {
                (y_coords[i - 1] + y_px) * 0.5
            };
            let y_max_px = if i + 1 == y_coords.len() {
                plot_y.1
            } else {
                (y_px + y_coords[i + 1]) * 0.5
            };
            let y_label = x_labels.get(i).cloned().unwrap_or_else(|| format!("#{i}"));
            let values = series
                .iter()
                .filter_map(|s| s.data.get(i).map(|pt| (s.name.clone(), pt.y)))
                .collect();
            buckets.push(HorizontalChartBucket {
                id: format!("{}:{y_label}", input.id),
                y_px,
                y_min: y_min_px.min(y_max_px),
                y_max: y_min_px.max(y_max_px),
                y_label,
                values,
            });
        }

        root.present()
            .map_err(|e| format!("plotters hbar present failed: {e}"))?;

        ChartHoverIndex::HorizontalBuckets {
            plot_x,
            plot_y,
            buckets,
            unit: unit.map(ToString::to_string),
        }
    };

    Ok(SvgBundle {
        svg: svg_buf,
        width: w,
        height: h,
        hit_map: HitMap {
            targets: Vec::new(),
            chart_hover: Some(hover_index),
        },
    })
}

fn compute_bounds(
    chart_type: ChartType,
    series: &[ChartSeries],
    max_override: Option<f64>,
) -> (f64, f64, f64, f64, usize) {
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    let mut max_len = 0usize;

    for s in series {
        max_len = max_len.max(s.data.len());
        for (i, pt) in s.data.iter().enumerate() {
            let x = pt.x.unwrap_or(i as f64);
            x_min = x_min.min(x);
            x_max = x_max.max(x);
            y_min = y_min.min(pt.y);
            y_max = y_max.max(pt.y);
        }
    }
    if let Some(m) = max_override {
        y_max = y_max.max(m);
    }

    if chart_type == ChartType::Bar {
        x_min -= 0.5;
        x_max += 0.5;
        y_min = y_min.min(0.0);
    } else if (x_max - x_min).abs() < 1e-9 {
        x_min -= 1.0;
        x_max += 1.0;
    } else if chart_type == ChartType::Scatter {
        let pad_x = (x_max - x_min) * 0.08;
        x_min -= pad_x;
        x_max += pad_x;
    }

    if (y_max - y_min).abs() < 1e-9 {
        let pad = y_max.abs().max(1.0) * 0.2;
        y_min -= pad;
        y_max += pad;
    } else {
        let pad_y = (y_max - y_min) * 0.1;
        if chart_type != ChartType::Bar || y_min < 0.0 {
            y_min -= pad_y;
        }
        y_max += pad_y;
    }

    (x_min, x_max, y_min, y_max, max_len)
}

fn fmt_axis(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

fn fmt_value_label(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{v:.0}")
    } else if v.abs() >= 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChartPoint;

    fn sample_series() -> Vec<ChartSeries> {
        vec![
            ChartSeries {
                name: "p99".into(),
                color: None,
                data: vec![
                    ChartPoint { x: None, y: 12.0 },
                    ChartPoint { x: None, y: 24.5 },
                    ChartPoint { x: None, y: 18.0 },
                ],
            },
            ChartSeries {
                name: "p50".into(),
                color: Some("#34D399".into()),
                data: vec![
                    ChartPoint { x: None, y: 5.0 },
                    ChartPoint { x: None, y: 8.5 },
                    ChartPoint { x: None, y: 6.0 },
                ],
            },
        ]
    }

    #[test]
    fn renders_all_chart_types_in_light_and_dark_themes_with_hover() {
        let labels = vec!["t0".into(), "t1".into(), "t2".into()];
        let series = sample_series();

        for theme in [Theme::light(), Theme::dark()] {
            for chart_type in [
                ChartType::Line,
                ChartType::Area,
                ChartType::Bar,
                ChartType::HBar,
                ChartType::Scatter,
            ] {
                let bundle = render_chart_svg(
                    &ChartRenderInput {
                        id: "lat",
                        title: Some("Latency"),
                        chart_type,
                        x_labels: &labels,
                        x_unit: (chart_type == ChartType::HBar).then_some("seconds"),
                        y_unit: Some("ms"),
                        value_labels: chart_type == ChartType::HBar,
                        max_override: None,
                        series: &series,
                    },
                    &theme,
                    640,
                    360,
                )
                .expect("render chart");

                assert!(bundle.svg.contains("<svg"));
                assert!(bundle.svg.contains("Latency"));

                match bundle.hit_map.chart_hover.as_ref().expect("hover index") {
                    ChartHoverIndex::Buckets {
                        plot_y, buckets, ..
                    } => {
                        assert_eq!(buckets.len(), 3);
                        let mid_y = (plot_y.0 + plot_y.1) * 0.5;
                        let hit = bundle
                            .hit_map
                            .hit_at(buckets[1].x_px, mid_y)
                            .expect("hit middle bucket");
                        assert_eq!(hit.title, "t1");
                        assert!(hit.lines.iter().any(|l| l.contains("p99: 24.50 ms")));
                        assert!(hit.lines.iter().any(|l| l.contains("p50: 8.50 ms")));
                    }
                    ChartHoverIndex::HorizontalBuckets {
                        plot_x, buckets, ..
                    } => {
                        assert_eq!(buckets.len(), 3);
                        assert!(bundle.svg.contains("24.50"));
                        assert!(bundle.svg.contains("seconds"));
                        let mid_x = (plot_x.0 + plot_x.1) * 0.5;
                        let hit = bundle
                            .hit_map
                            .hit_at(mid_x, buckets[1].y_px)
                            .expect("hit middle hbar category");
                        assert_eq!(hit.title, "t1");
                        assert!(hit.lines.iter().any(|l| l.contains("p99: 24.50 seconds")));
                        assert!(hit.lines.iter().any(|l| l.contains("p50: 8.50 seconds")));
                    }
                    ChartHoverIndex::Scatter { points, .. } => {
                        assert_eq!(points.len(), 6);
                        let hit = bundle
                            .hit_map
                            .hit_at(points[1].px + 2.0, points[1].py - 2.0)
                            .expect("hit scatter point");
                        assert_eq!(hit.title, "p99");
                        assert!(hit.lines.iter().any(|l| l.contains("24.50 ms")));
                    }
                }
            }
        }
    }
}
