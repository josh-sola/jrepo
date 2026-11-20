use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use image::RgbaImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::render::raster::RasterizedGraphic;

const KITTY_CHUNK_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPixelSize {
    pub width: u16,
    pub height: u16,
}

impl Default for CellPixelSize {
    fn default() -> Self {
        Self {
            width: 9,
            height: 18,
        }
    }
}

pub fn detect_kitty_support() -> bool {
    if std::env::var("KITTY_WINDOW_ID").is_ok_and(|v| !v.trim().is_empty()) {
        return true;
    }
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let lower = term_program.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "rex" | "ghostty" | "wezterm") {
            return true;
        }
    }
    false
}

pub fn query_cell_pixel_size() -> CellPixelSize {
    if let Ok(ws) = crossterm::terminal::window_size()
        && ws.width > 0
        && ws.columns > 0
        && ws.height > 0
        && ws.rows > 0
    {
        return CellPixelSize {
            width: (ws.width / ws.columns).max(1),
            height: (ws.height / ws.rows).max(1),
        };
    }
    CellPixelSize::default()
}

#[derive(Debug, Clone)]
pub struct KittyPlacement {
    pub pane_id: String,
    pub content_hash: u64,
    pub rect: Rect,
    pub graphic: Arc<RasterizedGraphic>,
    pub crop: Option<(u32, u32)>,
}

pub struct KittyPresenter {
    enabled: bool,
    next_id: u32,
    pane_ids: HashMap<String, u32>,
    active: HashMap<u32, (u64, Rect)>,
}

impl KittyPresenter {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            next_id: 1,
            pane_ids: HashMap::new(),
            active: HashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn id_for(&mut self, pane_id: &str) -> u32 {
        if let Some(&id) = self.pane_ids.get(pane_id) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.pane_ids.insert(pane_id.to_string(), id);
        id
    }

    pub fn sync(&mut self, placements: &[KittyPlacement], out: &mut impl Write) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let mut visible_ids = HashSet::with_capacity(placements.len());
        let mut dirty: Vec<(u32, &KittyPlacement)> = Vec::new();

        for placement in placements {
            if placement.rect.width == 0 || placement.rect.height == 0 {
                continue;
            }
            let id = self.id_for(&placement.pane_id);
            visible_ids.insert(id);
            let unchanged = self
                .active
                .get(&id)
                .is_some_and(|&(h, r)| h == placement.content_hash && r == placement.rect);
            if !unchanged {
                dirty.push((id, placement));
            }
        }

        let stale_ids: Vec<u32> = self
            .active
            .keys()
            .copied()
            .filter(|id| !visible_ids.contains(id))
            .collect();

        if stale_ids.is_empty() && dirty.is_empty() {
            return Ok(());
        }

        out.write_all(b"\x1b[?2026h")?;

        for id in stale_ids {
            write!(out, "\x1b_Ga=d,d=i,i={id},q=2\x1b\\")?;
            self.active.remove(&id);
        }

        for (id, placement) in dirty {
            write_kitty_image(
                out,
                id,
                placement.rect,
                &placement.graphic.image,
                placement.crop,
            )?;
            self.active
                .insert(id, (placement.content_hash, placement.rect));
        }

        out.write_all(b"\x1b[?2026l")?;
        out.flush()
    }

    pub fn clear_all(&mut self, out: &mut impl Write) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.active.clear();
        out.write_all(b"\x1b[?2026h\x1b_Ga=d,d=A,q=2\x1b\\\x1b[?2026l")?;
        out.flush()
    }
}

fn write_kitty_image(
    out: &mut impl Write,
    image_id: u32,
    rect: Rect,
    image: &RgbaImage,
    crop: Option<(u32, u32)>,
) -> io::Result<()> {
    let img_w = image.width();
    let img_h = image.height();
    let (src_y, src_h) = match crop {
        Some((y, h)) => {
            let clamped_y = y.min(img_h.saturating_sub(1));
            let clamped_h = h.min(img_h.saturating_sub(clamped_y)).max(1);
            (clamped_y, clamped_h)
        }
        None => (0, img_h),
    };
    let row_bytes = (img_w as usize) * 4;
    let start_byte = (src_y as usize) * row_bytes;
    let end_byte = ((src_y + src_h) as usize) * row_bytes;
    let slice = &image.as_raw()[start_byte..end_byte.min(image.as_raw().len())];

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(slice)?;
    let compressed = encoder.finish()?;
    let encoded = BASE64.encode(compressed);

    let row = rect.y.saturating_add(1);
    let col = rect.x.saturating_add(1);
    write!(out, "\x1b[{row};{col}H")?;

    let bytes = encoded.as_bytes();
    let total_chunks = bytes.len().div_ceil(KITTY_CHUNK_LEN).max(1);
    for (idx, chunk) in bytes.chunks(KITTY_CHUNK_LEN).enumerate() {
        let more = u8::from(idx + 1 < total_chunks);
        if idx == 0 {
            // Kitty z < INT32_MIN/2 renders below non-default cell backgrounds so opaque tooltips occlude the image.
            write!(
                out,
                "\x1b_Ga=T,f=32,o=z,t=d,i={image_id},p=1,z=-1073741825,q=2,C=1,s={img_w},v={src_h},c={},r={},m={more};",
                rect.width, rect.height,
            )?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

pub fn render_half_blocks(image: &RgbaImage, area: Rect, buf: &mut Buffer) {
    render_half_blocks_cropped(image, 0, image.height(), area, buf);
}

pub fn render_half_blocks_cropped(
    image: &RgbaImage,
    src_y: u32,
    src_h: u32,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width == 0
        || area.height == 0
        || image.width() == 0
        || image.height() == 0
        || src_h == 0
    {
        return;
    }
    let img_w = image.width();
    let img_h = image.height();
    let clamped_y = src_y.min(img_h.saturating_sub(1));
    let clamped_h = src_h.min(img_h.saturating_sub(clamped_y)).max(1);
    let cols = u32::from(area.width);
    let sub_rows = u32::from(area.height) * 2;

    for row in 0..area.height {
        let py_top = clamped_y + ((u32::from(row) * 2 * clamped_h) / sub_rows).min(clamped_h - 1);
        let py_bot =
            clamped_y + (((u32::from(row) * 2 + 1) * clamped_h) / sub_rows).min(clamped_h - 1);
        for col in 0..area.width {
            let px = ((u32::from(col) * img_w) / cols).min(img_w - 1);
            let top = image.get_pixel(px, py_top).0;
            let bot = image.get_pixel(px, py_bot).0;
            if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                cell.set_char('▀')
                    .set_fg(rgba_to_color(top))
                    .set_bg(rgba_to_color(bot));
            }
        }
    }
}

fn rgba_to_color(px: [u8; 4]) -> Color {
    if px[3] == 0 {
        Color::Reset
    } else {
        Color::Rgb(px[0], px[1], px[2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn kitty_presenter_deduplicates_unchanged_frames_and_deletes_stale_ids() {
        let mut img = RgbaImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = Rgba([60, 120, 240, 255]);
        }
        let graphic = Arc::new(RasterizedGraphic {
            image: img,
            hit_map: Default::default(),
            transform: crate::svg::FitTransform {
                scale: 1.0,
                offset_x: 0.0,
                offset_y: 0.0,
            },
        });
        let rect = Rect::new(2, 3, 10, 5);
        let placement = KittyPlacement {
            pane_id: "arch".into(),
            content_hash: 42,
            rect,
            graphic,
            crop: None,
        };

        let mut presenter = KittyPresenter::new(true);
        let mut out = Vec::new();
        presenter
            .sync(std::slice::from_ref(&placement), &mut out)
            .unwrap();
        let first = String::from_utf8(out).unwrap();
        assert!(first.starts_with("\x1b[?2026h"));
        assert!(first.ends_with("\x1b[?2026l"));
        assert!(first.contains("i=1,p=1,z=-1073741825,q=2,C=1,s=8,v=8,c=10,r=5"));

        let mut second_out = Vec::new();
        presenter
            .sync(std::slice::from_ref(&placement), &mut second_out)
            .unwrap();
        assert!(second_out.is_empty());

        let mut third_out = Vec::new();
        presenter.sync(&[], &mut third_out).unwrap();
        let third = String::from_utf8(third_out).unwrap();
        assert!(third.contains("a=d,d=i,i=1,q=2"));

        let mut clear_out = Vec::new();
        presenter.clear_all(&mut clear_out).unwrap();
        assert!(
            String::from_utf8(clear_out)
                .unwrap()
                .contains("a=d,d=A,q=2")
        );
    }

    #[test]
    fn half_block_fallback_populates_buffer_cells() {
        let mut img = RgbaImage::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                let col = if y < 2 {
                    Rgba([255, 0, 0, 255])
                } else {
                    Rgba([0, 0, 255, 255])
                };
                img.put_pixel(x, y, col);
            }
        }
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        render_half_blocks(&img, area, &mut buf);
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(cell.symbol(), "▀");
        assert_eq!(cell.fg, Color::Rgb(255, 0, 0));
        assert_eq!(cell.bg, Color::Rgb(255, 0, 0));
        let bottom = buf.cell((0, 1)).unwrap();
        assert_eq!(bottom.fg, Color::Rgb(0, 0, 255));
    }
}
