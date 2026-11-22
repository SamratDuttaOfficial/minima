//! Procedural app icon: a teal rounded square with a white terminal prompt
//! (`>` chevron + cursor bar). Rendered with 4x4 supersampling, then written
//! as PNGs, a multi-size .ico (Windows) and an .icns (macOS).

use std::fs;
use std::path::Path;

use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder, RgbaImage};

fn seg_dist(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let (apx, apy) = (px - a.0, py - a.1);
    let t = ((apx * abx + apy * aby) / (abx * abx + aby * aby)).clamp(0.0, 1.0);
    let (cx, cy) = (a.0 + abx * t, a.1 + aby * t);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Signed distance to a rounded rectangle (negative inside).
fn rrect(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let (dx, dy) = ((px - cx).abs() - (hx - r), (py - cy).abs() - (hy - r));
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - r
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Colour + coverage at a point in unit coordinates, or None outside the tile.
fn sample(x: f32, y: f32) -> Option<[f32; 4]> {
    if rrect(x, y, 0.5, 0.5, 0.47, 0.47, 0.22) > 0.0 {
        return None;
    }
    // diagonal teal gradient (#25d9c9 -> #0a5f75) with a soft top-left sheen
    let t = ((x + y) * 0.5).clamp(0.0, 1.0);
    let mut c = [lerp(37.0, 10.0, t), lerp(217.0, 95.0, t), lerp(201.0, 117.0, t)];
    let sheen = (1.0 - ((x - 0.25).powi(2) + (y - 0.18).powi(2)).sqrt() * 1.8).clamp(0.0, 1.0) * 0.18;
    for v in c.iter_mut() {
        *v = lerp(*v, 255.0, sheen);
    }
    // soft shadow under the glyphs
    let sh = {
        let d1 = seg_dist(x, y - 0.025, (0.30, 0.31), (0.51, 0.50)).min(seg_dist(x, y - 0.025, (0.51, 0.50), (0.30, 0.69)));
        let d2 = rrect(x, y - 0.025, 0.62, 0.705, 0.115, 0.035, 0.03);
        (1.0 - (d1.min(d2.max(0.0)) - 0.045) / 0.05).clamp(0.0, 1.0) * 0.28
    };
    for v in c.iter_mut() {
        *v *= 1.0 - sh;
    }
    // chevron
    let d_chev = seg_dist(x, y, (0.30, 0.31), (0.51, 0.50)).min(seg_dist(x, y, (0.51, 0.50), (0.30, 0.69))) - 0.052;
    // cursor bar
    let d_bar = rrect(x, y, 0.62, 0.705, 0.115, 0.035, 0.03);
    if d_chev.min(d_bar) < 0.0 {
        c = [255.0, 255.0, 255.0];
    }
    Some([c[0], c[1], c[2], 255.0])
}

pub fn render(size: u32) -> RgbaImage {
    let mut img = RgbaImage::new(size, size);
    const SS: u32 = 4;
    for py in 0..size {
        for px in 0..size {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    if let Some(c) = sample(x, y) {
                        r += c[0];
                        g += c[1];
                        b += c[2];
                        a += 1.0;
                    }
                }
            }
            let n = (SS * SS) as f32;
            if a > 0.0 {
                img.put_pixel(px, py, image::Rgba([(r / a) as u8, (g / a) as u8, (b / a) as u8, (a / n * 255.0) as u8]));
            }
        }
    }
    img
}

fn png_bytes(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8).unwrap();
    out
}

fn ico(sizes: &[u32]) -> Vec<u8> {
    let pngs: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&s| (s, png_bytes(&render(s)))).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len();
    for (s, data) in &pngs {
        let b = if *s >= 256 { 0u8 } else { *s as u8 };
        out.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, d) in pngs {
        out.extend_from_slice(&d);
    }
    out
}

fn icns() -> Vec<u8> {
    let entries: [(&[u8; 4], u32); 5] = [(b"icp5", 32), (b"icp6", 64), (b"ic07", 128), (b"ic08", 256), (b"ic09", 512)];
    let mut body = Vec::new();
    for (tag, size) in entries {
        let data = png_bytes(&render(size));
        body.extend_from_slice(tag);
        body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let data = png_bytes(&render(1024));
    body.extend_from_slice(b"ic10");
    body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
    body.extend_from_slice(&data);
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#25d9c9"/>
      <stop offset="1" stop-color="#0a5f75"/>
    </linearGradient>
  </defs>
  <rect x="14" y="14" width="484" height="484" rx="113" fill="url(#g)"/>
  <polyline points="154,159 261,256 154,353" fill="none" stroke="#fff" stroke-width="53" stroke-linecap="round" stroke-linejoin="round"/>
  <rect x="261" y="343" width="118" height="36" rx="15" fill="#fff"/>
</svg>
"##;

pub fn generate(out: &Path) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    for s in [16u32, 32, 48, 64, 128, 256, 512] {
        fs::write(out.join(format!("minima-{s}.png")), png_bytes(&render(s))).map_err(|e| e.to_string())?;
    }
    fs::write(out.join("minima.ico"), ico(&[16, 24, 32, 48, 64, 128, 256])).map_err(|e| e.to_string())?;
    fs::write(out.join("minima.icns"), icns()).map_err(|e| e.to_string())?;
    fs::write(out.join("minima.svg"), SVG).map_err(|e| e.to_string())?;
    println!("  wrote icons to {}", out.display());
    Ok(())
}
