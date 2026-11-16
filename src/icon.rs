//! Icons, drawn in code (no image files).
//!
//! - The app icon: a dark rounded tile with a "c" built from seven blocks, the same blocks the
//!   progress window stacks.
//! - Right-click menu glyphs: a 2×2 block "size meter" (1–4 blocks filled, smallest to largest
//!   size) and a sliders glyph for settings, in a light and a dark variant to match the menu.
//!
//! Geometry is laid out in whole pixels for each size, so small icons stay crisp.
//! This file is also compiled by `build.rs` (to embed the icon in the exe), so it must not use
//! anything from the rest of the crate.

#![allow(dead_code)]

/// Signed distance to an axis-aligned rounded rectangle (negative inside).
fn rect_sdf(x: f32, y: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32) -> f32 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hx, hy) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let r = r.min(hx).min(hy);
    let qx = (x - cx).abs() - (hx - r);
    let qy = (y - cy).abs() - (hy - r);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// Rasterizes `shade(x, y) -> (gray, alpha)` (pixel coordinates) into straight-alpha RGBA,
/// supersampled for smooth edges.
fn raster(size: u32, shade: impl Fn(f32, f32) -> (f32, f32)) -> Vec<u8> {
    let ss = if size <= 48 { 8 } else { 4 };
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut premul, mut alpha) = (0.0, 0.0);
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = px as f32 + (sx as f32 + 0.5) / ss as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / ss as f32;
                    let (g, a) = shade(x, y);
                    premul += g * a;
                    alpha += a;
                }
            }
            let a = alpha / (ss * ss) as f32;
            let g = if alpha > 0.0 { premul / alpha } else { 0.0 };
            let g = (g * 255.0).round() as u8;
            out.extend_from_slice(&[g, g, g, (a * 255.0).round() as u8]);
        }
    }
    out
}

/// Pixel-snapped square cells of an `n`×`n` grid centered in `size`: (cell, gap, x0, y0).
/// Leftover odd pixels go to the left/top for x/y according to `bias`.
fn grid(size: u32, n: u32, cell: f32, gap: f32, bias: (bool, bool)) -> (f32, f32, f32, f32) {
    let s = size as f32;
    let cell = (s * cell).round().max(1.0);
    let gap = (s * gap).round().max(1.0);
    let span = n as f32 * cell + (n - 1) as f32 * gap;
    let free = (s - span) / 2.0;
    let x0 = if bias.0 { free.ceil() } else { free.floor() };
    let y0 = if bias.1 { free.ceil() } else { free.floor() };
    (cell, gap, x0, y0)
}

// ---------------------------------------------------------------------------------------------
// App icon
// ---------------------------------------------------------------------------------------------

/// The blocks of the "c", as (column, row) in a 3×3 grid: everything but the center and the
/// middle of the right column.
const C_BLOCKS: [(u32, u32); 7] = [(0, 0), (1, 0), (2, 0), (0, 1), (0, 2), (1, 2), (2, 2)];

/// Straight-alpha RGBA pixels of the app icon.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let tile_r = s * 0.23;
    // The "c" is open on the right, so it sits half a pixel right of center to look centered.
    let (cell, gap, x0, y0) = grid(size, 3, 0.165, 0.05, (true, false));
    // Optical centering: the open right side makes the "c" look left-heavy.
    let x0 = x0 + (s * 0.018).round();
    let block_r = if size >= 32 { cell * 0.16 } else { 0.0 };
    let blocks: Vec<(f32, f32, f32, f32)> = C_BLOCKS
        .iter()
        .map(|&(c, r)| {
            let bx = x0 + c as f32 * (cell + gap);
            let by = y0 + r as f32 * (cell + gap);
            (bx, by, bx + cell, by + cell)
        })
        .collect();
    raster(size, |x, y| {
        let tile = rect_sdf(x, y, (0.0, 0.0, s, s), tile_r);
        if tile > 0.0 {
            return (0.0, 0.0);
        }
        let v = y / s;
        if blocks.iter().any(|&b| rect_sdf(x, y, b, block_r) <= 0.0) {
            // Blocks: white with the faintest top-to-bottom falloff.
            return (0.97 - 0.05 * v, 1.0);
        }
        // Tile: soft top-to-bottom gradient with a faint highlight along the top edge.
        let base = 0.20 - 0.11 * v;
        let rim = if tile > -(s * 0.018).max(1.0) && v < 0.5 { 0.07 * (1.0 - v * 2.0) } else { 0.0 };
        (base + rim, 1.0)
    })
}

/// A multi-size .ico of the app icon, for Explorer, Apps & Features and the exe itself.
pub fn ico_bytes() -> Vec<u8> {
    ico(&[16, 20, 24, 32, 40, 48, 64, 256], rgba)
}

// ---------------------------------------------------------------------------------------------
// Menu glyphs
// ---------------------------------------------------------------------------------------------

/// What a right-click menu entry shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// 2×2 blocks with 1–4 filled.
    Size(u8),
    Settings,
}

impl Glyph {
    pub fn file_stem(self) -> String {
        match self {
            Glyph::Size(level) => format!("size{level}"),
            Glyph::Settings => "settings".into(),
        }
    }
}

/// Which blocks of the 2×2 meter are filled at `level` (1–4). They fill like the progress grid:
/// right to left, bottom to top. Indexed [row][column], row 0 on top.
pub fn meter_filled(level: u8) -> [[bool; 2]; 2] {
    let order = [(1, 1), (1, 0), (0, 1), (0, 0)];
    let mut filled = [[false; 2]; 2];
    for &(r, c) in order.iter().take(level.clamp(1, 4) as usize) {
        filled[r][c] = true;
    }
    filled
}

/// Straight-alpha RGBA of a menu glyph; `light_menu` = dark ink for light-themed menus.
pub fn glyph_rgba(glyph: Glyph, size: u32, light_menu: bool) -> Vec<u8> {
    let ink = if light_menu { 0.10 } else { 0.95 };
    let dim = if light_menu { 0.26 } else { 0.30 };
    let s = size as f32;
    match glyph {
        Glyph::Size(level) => {
            let (cell, gap, x0, y0) = grid(size, 2, 0.32, 0.105, (false, false));
            let r = (cell * 0.18).max(if size >= 20 { 1.0 } else { 0.6 });
            let filled = meter_filled(level);
            raster(size, |x, y| {
                for (row, cols) in filled.iter().enumerate() {
                    for (col, &on) in cols.iter().enumerate() {
                        let bx = x0 + col as f32 * (cell + gap);
                        let by = y0 + row as f32 * (cell + gap);
                        if rect_sdf(x, y, (bx, by, bx + cell, by + cell), r) <= 0.0 {
                            return (ink, if on { 1.0 } else { dim });
                        }
                    }
                }
                (0.0, 0.0)
            })
        }
        Glyph::Settings => {
            // Two slider tracks with knobs.
            let line = (s / 16.0).round().max(1.0);
            let (left, right) = ((s * 0.1).round(), s - (s * 0.1).round());
            let rows = [
                ((s * 0.3).round(), (s * 0.62).floor() + line / 2.0),
                ((s * 0.7).round() - line, (s * 0.38).floor() + line / 2.0),
            ];
            let knob = s * 0.13;
            raster(size, |x, y| {
                for &(ty, kx) in &rows {
                    let ky = ty + line / 2.0;
                    let d = ((x - kx).powi(2) + (y - ky).powi(2)).sqrt();
                    if d <= knob || rect_sdf(x, y, (left, ty, right, ty + line), line / 2.0) <= 0.0 {
                        return (ink, 1.0);
                    }
                }
                (0.0, 0.0)
            })
        }
    }
}

pub fn glyph_ico(glyph: Glyph, light_menu: bool) -> Vec<u8> {
    ico(&[16, 20, 24, 28, 32, 40, 48, 64], |s| glyph_rgba(glyph, s, light_menu))
}

// ---------------------------------------------------------------------------------------------
// .ico encoding
// ---------------------------------------------------------------------------------------------

fn ico(sizes: &[u32], render: impl Fn(u32) -> Vec<u8>) -> Vec<u8> {
    // Large frames are stored as PNG (supported since Vista), small ones as BMP for compatibility.
    let images: Vec<Vec<u8>> =
        sizes.iter().map(|&s| if s >= 128 { png(s, &render(s)) } else { bmp_entry(s, &render(s)) }).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

fn bmp_entry(size: u32, px: &[u8]) -> Vec<u8> {
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::new();
    // BITMAPINFOHEADER; height is doubled to account for the AND mask.
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    out.extend(std::iter::repeat_n(0u8, (mask_row * size) as usize));
    out
}

/// Minimal RGBA PNG encoder.
fn png(size: u32, px: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(px.len() + size as usize);
    for row in px.chunks(size as usize * 4) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in
        [(b"IHDR", ihdr), (b"IDAT", miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9)), (b"IEND", Vec::new())]
    {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(&data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_render_at_every_size() {
        for s in [16, 20, 24, 32, 48, 256] {
            assert_eq!(rgba(s).len(), (s * s * 4) as usize);
            for g in [Glyph::Size(1), Glyph::Size(4), Glyph::Settings] {
                let px = glyph_rgba(g, s, false);
                assert!(px.chunks(4).any(|p| p[3] == 255), "{g:?} at {s}px is empty");
            }
        }
    }

    #[test]
    fn meter_fills_right_to_left_bottom_to_top() {
        assert_eq!(meter_filled(1), [[false, false], [false, true]]);
        assert_eq!(meter_filled(2), [[false, false], [true, true]]);
        assert_eq!(meter_filled(4), [[true, true], [true, true]]);
    }

    /// Writes preview PNG-able BMP sheets to %TEMP%\comprimo-icons:
    /// `cargo test write_icon_previews -- --ignored`
    #[test]
    #[ignore]
    fn write_icon_previews() {
        let dir = std::env::temp_dir().join("comprimo-icons");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("app.ico"), ico_bytes()).unwrap();
        for light in [false, true] {
            for g in [Glyph::Size(1), Glyph::Size(2), Glyph::Size(3), Glyph::Size(4), Glyph::Settings] {
                let name = format!("{}-{}.ico", if light { "light" } else { "dark" }, g.file_stem());
                std::fs::write(dir.join(name), glyph_ico(g, light)).unwrap();
            }
        }
    }
}
