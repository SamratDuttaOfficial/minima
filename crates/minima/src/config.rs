//! Configuration (a tiny `key = value` file) and on-disk locations.

use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageMode {
    /// Classic ASCII brightness ramp.
    Ascii,
    /// Half-block cells (2 pixels per cell) in truecolor.
    Blocks,
    /// Braille dots (8 pixels per cell) — the densest option.
    Braille,
}

impl ImageMode {
    pub fn next(self) -> ImageMode {
        match self {
            ImageMode::Ascii => ImageMode::Blocks,
            ImageMode::Blocks => ImageMode::Braille,
            ImageMode::Braille => ImageMode::Ascii,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            ImageMode::Ascii => "ascii",
            ImageMode::Blocks => "blocks",
            ImageMode::Braille => "braille",
        }
    }
    fn parse(s: &str) -> Option<ImageMode> {
        match s.trim().to_lowercase().as_str() {
            "ascii" => Some(ImageMode::Ascii),
            "blocks" | "block" | "half" => Some(ImageMode::Blocks),
            "braille" => Some(ImageMode::Braille),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub home: String,
    /// `{q}` is replaced with the URL-encoded query.
    pub search: String,
    pub image_mode: ImageMode,
    pub color: bool,
    /// Try to shrink the terminal font while viewing images / video.
    pub font_shrink: bool,
    pub download_dir: PathBuf,
    pub data_dir: PathBuf,
    pub config_path: PathBuf,
    pub user_agent: String,
    pub video_fps: u32,
}

impl Config {
    pub fn load() -> Config {
        let data_dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("minima");
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("minima");
        let download_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("."));
        let term = std::env::var("TERM").unwrap_or_default();
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let color = std::env::var_os("NO_COLOR").is_none()
            && (colorterm.contains("truecolor") || colorterm.contains("24bit") || cfg!(windows) || term.contains("256") || term.contains("kitty") || term.contains("alacritty"));
        let font_shrink = term.starts_with("xterm") || term.starts_with("rxvt") || term.contains("mlterm");
        let mut c = Config {
            home: "https://www.google.com/".into(),
            search: "https://www.google.com/search?q={q}".into(),
            image_mode: ImageMode::Braille,
            color,
            font_shrink,
            download_dir,
            data_dir,
            config_path: config_dir.join("config"),
            user_agent: "Mozilla/5.0 (compatible; Minima/0.1; +terminal) Lynx/2.9.0 libwww-FM/2.14".into(),
            video_fps: 12,
        };
        if let Ok(text) = fs::read_to_string(&c.config_path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let v = v.trim().trim_matches('"');
                    match k.trim() {
                        "home" => c.home = v.to_string(),
                        "search" => c.search = v.to_string(),
                        "image_mode" => {
                            if let Some(m) = ImageMode::parse(v) {
                                c.image_mode = m
                            }
                        }
                        "color" => c.color = matches!(v, "1" | "true" | "yes" | "on"),
                        "font_shrink" => c.font_shrink = matches!(v, "1" | "true" | "yes" | "on"),
                        "download_dir" => c.download_dir = PathBuf::from(v),
                        "user_agent" => c.user_agent = v.to_string(),
                        "video_fps" => c.video_fps = v.parse().unwrap_or(12).clamp(2, 30),
                        _ => {}
                    }
                }
            }
        }
        let _ = fs::create_dir_all(&c.data_dir);
        c
    }

    /// Write a commented default config if none exists.
    pub fn ensure_default_file(&self) {
        if self.config_path.exists() {
            return;
        }
        if let Some(dir) = self.config_path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let text = format!(
            "# Minima configuration\n\
             home = {}\n\
             search = {}\n\
             # ascii | blocks | braille\n\
             image_mode = {}\n\
             color = {}\n\
             font_shrink = {}\n\
             video_fps = {}\n\
             # download_dir = /path/to/dir\n",
            self.home,
            self.search,
            self.image_mode.name(),
            self.color,
            self.font_shrink,
            self.video_fps
        );
        let _ = fs::write(&self.config_path, text);
    }
}
