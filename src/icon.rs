//! The app icon, drawn in code: a dark rounded tile with a white square inside a white frame
//! ("something big squeezed into something small").

/// Coverage of the icon shape at normalized coordinates, returned as (gray, alpha).
fn shade(u: f32, v: f32) -> (f32, f32) {
    // Signed distance to a rounded box centered at 0.5 with half-size `h` and corner radius `r`.
    let rounded_box = |h: f32, r: f32| {
        let qx = (u - 0.5).abs() - (h - r);
        let qy = (v - 0.5).abs() - (h - r);
        (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
    };
    if rounded_box(0.5, 0.23) > 0.0 {
        return (0.0, 0.0);
    }
    let frame = rounded_box(0.28, 0.06) <= 0.0 && rounded_box(0.215, 0.03) > 0.0;
    let core = rounded_box(0.10, 0.025) <= 0.0;
    if frame || core {
        return (0.96, 1.0);
    }
    // Tile: soft top-to-bottom gradient with a faint highlight along the top edge.
    let base = 0.20 - 0.11 * v;
    let rim = if rounded_box(0.5, 0.23) > -0.018 && v < 0.5 { 0.07 * (1.0 - v * 2.0) } else { 0.0 };
    (base + rim, 1.0)
}

/// Straight-alpha RGBA pixels, 4×4 supersampled for smooth edges.
pub fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut premul, mut alpha) = (0.0, 0.0);
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (x as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let (g, a) = shade(u, v);
                    premul += g * a;
                    alpha += a;
                }
            }
            let n = (SS * SS) as f32;
            let a = alpha / n;
            let g = if alpha > 0.0 { premul / alpha } else { 0.0 };
            let g = (g * 255.0).round() as u8;
            out.extend_from_slice(&[g, g, g, (a * 255.0).round() as u8]);
        }
    }
    out
}

/// A multi-size .ico file (32-bit BMP entries) for Explorer and Apps & Features.
pub fn ico_bytes() -> Vec<u8> {
    let sizes = [16u32, 20, 24, 32, 48, 64, 256];
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| bmp_entry(s)).collect();
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

fn bmp_entry(size: u32) -> Vec<u8> {
    let px = rgba(size);
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
