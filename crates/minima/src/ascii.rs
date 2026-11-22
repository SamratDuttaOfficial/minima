//! Image → terminal art. Three densities:
//!
//! * `Ascii`   — brightness ramp, 1 pixel per cell
//! * `Blocks`  — `▀` half blocks, 2 pixels per cell, truecolor
//! * `Braille` — `⠿` braille cells, 8 pixels per cell, dithered; the densest
//!
//! Because braille/blocks pack many pixels into one cell, the picture stays
//! sharp even when the terminal's font is large — no font change required.

use image::{imageops::FilterType, DynamicImage, RgbImage};

use crate::config::ImageMode;

const RAMP: &[u8] = b" .'`^\",:;Il!i><~+_-?][}{1)(|/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";
const RAMP_SHORT: &[u8] = b" .:-=+*#%@";

/// Pixel grid (w, h) for a mode occupying `cols` x `rows` terminal cells.
pub fn pixel_grid(mode: ImageMode, cols: usize, rows: usize) -> (usize, usize) {
    match mode {
        ImageMode::Ascii => (cols, rows),
        ImageMode::Blocks => (cols, rows * 2),
        ImageMode::Braille => (cols * 2, rows * 4),
    }
}

/// How many terminal cells (cols, rows) an `iw`x`ih` image needs to fit inside
/// `max_cols`x`max_rows` while keeping its aspect ratio.
pub fn fit_cells(mode: ImageMode, iw: u32, ih: u32, max_cols: usize, max_rows: usize) -> (usize, usize) {
    let (iw, ih) = (iw.max(1) as f32, ih.max(1) as f32);
    // A terminal cell is roughly twice as tall as wide.
    let cell_aspect = match mode {
        ImageMode::Ascii => 2.0,
        ImageMode::Blocks | ImageMode::Braille => 2.0,
    };
    // width in cells for a given row count: cols = rows * cell_aspect * iw/ih
    let mut cols = max_cols as f32;
    let mut rows = cols * ih / iw / cell_aspect;
    if rows > max_rows as f32 {
        rows = max_rows as f32;
        cols = rows * cell_aspect * iw / ih;
    }
    ((cols.round() as usize).clamp(1, max_cols.max(1)), (rows.round() as usize).clamp(1, max_rows.max(1)))
}

pub fn render_image(img: &DynamicImage, mode: ImageMode, color: bool, max_cols: usize, max_rows: usize) -> Vec<String> {
    let (cols, rows) = fit_cells(mode, img.width(), img.height(), max_cols, max_rows);
    let (pw, ph) = pixel_grid(mode, cols, rows);
    // thumbnail first (fast box filter) for huge sources, then precise resize.
    let small = if img.width() > (pw as u32) * 4 || img.height() > (ph as u32) * 4 {
        img.thumbnail((pw as u32) * 2, (ph as u32) * 2)
    } else {
        img.clone()
    };
    let rgb = flatten_alpha(&small).resize_exact(pw as u32, ph as u32, FilterType::Lanczos3).to_rgb8();
    convert(rgb.as_raw(), pw, ph, mode, color)
}

fn flatten_alpha(img: &DynamicImage) -> DynamicImage {
    if !img.color().has_alpha() {
        return img.clone();
    }
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as f32 / 255.0;
        // composite over dark grey so transparent logos stay legible on dark terminals
        let bg = 24.0;
        out.put_pixel(
            x,
            y,
            image::Rgb([
                (p[0] as f32 * a + bg * (1.0 - a)) as u8,
                (p[1] as f32 * a + bg * (1.0 - a)) as u8,
                (p[2] as f32 * a + bg * (1.0 - a)) as u8,
            ]),
        );
    }
    DynamicImage::ImageRgb8(out)
}

#[inline]
fn luma(r: u8, g: u8, b: u8) -> f32 {
    0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
}

/// Convert a packed RGB24 buffer already sized to the mode's pixel grid.
pub fn convert(rgb: &[u8], pw: usize, ph: usize, mode: ImageMode, color: bool) -> Vec<String> {
    match mode {
        ImageMode::Ascii => to_ascii(rgb, pw, ph, color),
        ImageMode::Blocks => to_blocks(rgb, pw, ph, color),
        ImageMode::Braille => to_braille(rgb, pw, ph, color),
    }
}

fn px(rgb: &[u8], pw: usize, x: usize, y: usize) -> (u8, u8, u8) {
    let i = (y * pw + x) * 3;
    (rgb[i], rgb[i + 1], rgb[i + 2])
}

struct Painter {
    out: String,
    cur_fg: Option<(u8, u8, u8)>,
    cur_bg: Option<(u8, u8, u8)>,
}

impl Painter {
    fn new() -> Self {
        Painter { out: String::new(), cur_fg: None, cur_bg: None }
    }
    fn fg(&mut self, c: (u8, u8, u8)) {
        // quantise lightly so runs of near-identical colour share one escape
        let q = |v: u8| (v & 0xF8) | 4;
        let c = (q(c.0), q(c.1), q(c.2));
        if self.cur_fg != Some(c) {
            use std::fmt::Write;
            let _ = write!(self.out, "\x1b[38;2;{};{};{}m", c.0, c.1, c.2);
            self.cur_fg = Some(c);
        }
    }
    fn bg(&mut self, c: (u8, u8, u8)) {
        let q = |v: u8| (v & 0xF8) | 4;
        let c = (q(c.0), q(c.1), q(c.2));
        if self.cur_bg != Some(c) {
            use std::fmt::Write;
            let _ = write!(self.out, "\x1b[48;2;{};{};{}m", c.0, c.1, c.2);
            self.cur_bg = Some(c);
        }
    }
    fn finish(mut self) -> String {
        if self.cur_fg.is_some() || self.cur_bg.is_some() {
            self.out.push_str("\x1b[0m");
        }
        self.out
    }
}

fn levels(rgb: &[u8]) -> (f32, f32) {
    // auto-contrast: 2nd..98th percentile of luma
    let mut hist = [0u32; 256];
    let mut n = 0u32;
    for p in rgb.chunks_exact(3) {
        hist[luma(p[0], p[1], p[2]) as usize] += 1;
        n += 1;
    }
    if n == 0 {
        return (0.0, 255.0);
    }
    let (mut lo, mut hi) = (0usize, 255usize);
    let mut acc = 0;
    for (i, h) in hist.iter().enumerate() {
        acc += h;
        if acc * 50 >= n {
            lo = i;
            break;
        }
    }
    acc = 0;
    for i in (0..256).rev() {
        acc += hist[i];
        if acc * 50 >= n {
            hi = i;
            break;
        }
    }
    if hi <= lo + 8 {
        (0.0, 255.0)
    } else {
        (lo as f32, hi as f32)
    }
}

fn to_ascii(rgb: &[u8], pw: usize, ph: usize, color: bool) -> Vec<String> {
    let (lo, hi) = levels(rgb);
    let ramp = if color { RAMP_SHORT } else { RAMP };
    let mut lines = Vec::with_capacity(ph);
    for y in 0..ph {
        let mut p = Painter::new();
        for x in 0..pw {
            let (r, g, b) = px(rgb, pw, x, y);
            let l = ((luma(r, g, b) - lo) / (hi - lo)).clamp(0.0, 1.0);
            let ch = ramp[(l * (ramp.len() - 1) as f32).round() as usize] as char;
            if color {
                // boost saturation a little so the thin glyphs stay visible
                p.fg(boost((r, g, b), l));
            }
            p.out.push(ch);
        }
        lines.push(p.finish());
    }
    lines
}

fn boost(c: (u8, u8, u8), l: f32) -> (u8, u8, u8) {
    let f = if l < 0.3 { 1.0 + (0.3 - l) * 2.5 } else { 1.0 };
    let m = |v: u8| ((v as f32 * f).min(255.0)) as u8;
    (m(c.0).max(40), m(c.1).max(40), m(c.2).max(40))
}

fn to_blocks(rgb: &[u8], pw: usize, ph: usize, color: bool) -> Vec<String> {
    let rows = ph / 2;
    let mut lines = Vec::with_capacity(rows);
    for cy in 0..rows {
        let mut p = Painter::new();
        for x in 0..pw {
            let top = px(rgb, pw, x, cy * 2);
            let bot = px(rgb, pw, x, cy * 2 + 1);
            if color {
                p.fg(top);
                p.bg(bot);
                p.out.push('▀');
            } else {
                let lt = luma(top.0, top.1, top.2);
                let lb = luma(bot.0, bot.1, bot.2);
                let t = lt > 127.0;
                let b = lb > 127.0;
                p.out.push(match (t, b) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                });
            }
        }
        lines.push(p.finish());
    }
    lines
}

fn to_braille(rgb: &[u8], pw: usize, ph: usize, color: bool) -> Vec<String> {
    let (lo, hi) = levels(rgb);
    // luma grid with Floyd–Steinberg error diffusion
    let mut l: Vec<f32> = Vec::with_capacity(pw * ph);
    for p in rgb.chunks_exact(3) {
        l.push(((luma(p[0], p[1], p[2]) - lo) / (hi - lo)).clamp(0.0, 1.0));
    }
    let mut bits = vec![false; pw * ph];
    for y in 0..ph {
        for x in 0..pw {
            let i = y * pw + x;
            let old = l[i];
            let new = if old > 0.5 { 1.0 } else { 0.0 };
            bits[i] = new > 0.5;
            let err = old - new;
            if x + 1 < pw {
                l[i + 1] += err * 7.0 / 16.0;
            }
            if y + 1 < ph {
                if x > 0 {
                    l[i + pw - 1] += err * 3.0 / 16.0;
                }
                l[i + pw] += err * 5.0 / 16.0;
                if x + 1 < pw {
                    l[i + pw + 1] += err * 1.0 / 16.0;
                }
            }
        }
    }
    let cols = pw / 2;
    let rows = ph / 4;
    // braille dot bit for (dx, dy)
    const DOT: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    let mut lines = Vec::with_capacity(rows);
    for cy in 0..rows {
        let mut p = Painter::new();
        for cx in 0..cols {
            let mut code = 0u32;
            let (mut sr, mut sg, mut sb, mut n) = (0u32, 0u32, 0u32, 0u32);
            for dx in 0..2 {
                for dy in 0..4 {
                    let x = cx * 2 + dx;
                    let y = cy * 4 + dy;
                    let (r, g, b) = px(rgb, pw, x, y);
                    sr += r as u32;
                    sg += g as u32;
                    sb += b as u32;
                    n += 1;
                    if bits[y * pw + x] {
                        code |= DOT[dx][dy];
                    }
                }
            }
            if color {
                let avg = ((sr / n) as u8, (sg / n) as u8, (sb / n) as u8);
                let lum = luma(avg.0, avg.1, avg.2) / 255.0;
                p.fg(boost(avg, lum));
            }
            p.out.push(char::from_u32(0x2800 + code).unwrap_or(' '));
        }
        lines.push(p.finish());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_and_fit() {
        assert_eq!(pixel_grid(ImageMode::Braille, 10, 5), (20, 20));
        let (c, r) = fit_cells(ImageMode::Braille, 100, 100, 80, 24);
        assert!(r <= 24 && c <= 80);
        // square image on 2:1 cells → cols ≈ 2*rows
        assert!((c as i32 - 2 * r as i32).abs() <= 2);
    }

    #[test]
    fn renders_all_modes() {
        let mut img = RgbImage::new(64, 64);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 4) as u8, (y * 4) as u8, 128]);
        }
        let d = DynamicImage::ImageRgb8(img);
        for m in [ImageMode::Ascii, ImageMode::Blocks, ImageMode::Braille] {
            for color in [false, true] {
                let lines = render_image(&d, m, color, 40, 12);
                assert!(!lines.is_empty());
                assert!(lines.len() <= 12);
            }
        }
    }
}
