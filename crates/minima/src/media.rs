//! Video in the terminal: ffmpeg decodes, we turn every frame into ASCII /
//! block / braille art, ffplay supplies the sound. Animated GIFs work with
//! no external tools at all. YouTube goes through yt-dlp when available.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use url::Url;

use crate::ascii;
use crate::config::{Config, ImageMode};
use crate::fetch::Fetcher;
use crate::util;

pub struct Tools {
    pub ffmpeg: Option<PathBuf>,
    pub ffplay: Option<PathBuf>,
    pub ytdlp: Option<PathBuf>,
}

pub fn tools() -> Tools {
    Tools { ffmpeg: util::find_exe("ffmpeg"), ffplay: util::find_exe("ffplay"), ytdlp: util::find_exe("yt-dlp") }
}

// ---------------------------------------------------------------------------
// YouTube helpers
// ---------------------------------------------------------------------------

pub fn is_youtube_host(host: &str) -> bool {
    let h = host.trim_start_matches("www.").trim_start_matches("m.");
    h == "youtube.com" || h == "youtu.be" || h == "music.youtube.com" || h == "youtube-nocookie.com"
}

pub fn youtube_id(u: &Url) -> Option<String> {
    let host = u.host_str()?;
    if !is_youtube_host(host) {
        return None;
    }
    if host.contains("youtu.be") {
        return u.path_segments()?.next().filter(|s| !s.is_empty()).map(|s| s.to_string());
    }
    let mut segs = u.path_segments()?;
    match segs.next() {
        Some("watch") => u.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.to_string()),
        Some("shorts") | Some("embed") | Some("live") | Some("v") => segs.next().map(|s| s.to_string()),
        _ => None,
    }
}

pub fn embed_to_watch(src: &str) -> String {
    if let Ok(u) = Url::parse(src) {
        if let Some(id) = youtube_id(&u) {
            return format!("https://www.youtube.com/watch?v={id}");
        }
    }
    src.to_string()
}

pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// Resolve a page URL into something ffmpeg can open.
pub fn resolve_stream(url: &str, tools: &Tools) -> Result<String, String> {
    let parsed = Url::parse(url).ok();
    let needs_ytdlp = parsed
        .as_ref()
        .and_then(|u| u.host_str())
        .map(|h| is_youtube_host(h) || h.contains("vimeo.com") || h.contains("dailymotion.com") || h.contains("twitch.tv") || h.contains("tiktok.com"))
        .unwrap_or(false);
    if !needs_ytdlp {
        return Ok(url.to_string());
    }
    let yt = tools.ytdlp.as_ref().ok_or_else(|| {
        "Playing YouTube needs yt-dlp. Install it (https://github.com/yt-dlp/yt-dlp) or re-run the Minima installer with --with-media.".to_string()
    })?;
    let out = Command::new(yt)
        .args(["-g", "--no-playlist", "--no-warnings", "-f", "18/best[height<=480][acodec!=none][vcodec!=none]/worst[acodec!=none][vcodec!=none]/worst", url])
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| format!("yt-dlp failed to start: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("yt-dlp: {}", err.lines().last().unwrap_or("could not resolve stream")));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.starts_with("http"))
        .map(|s| s.to_string())
        .ok_or_else(|| "yt-dlp returned no stream URL".to_string())
}

/// Spawn yt-dlp to download a video; returns the child so the caller can stream progress.
pub fn spawn_ytdlp_download(url: &str, dir: &std::path::Path, tools: &Tools) -> Result<Child, String> {
    let yt = tools
        .ytdlp
        .as_ref()
        .ok_or_else(|| "downloading this video needs yt-dlp (not found)".to_string())?;
    let mut cmd = Command::new(yt);
    cmd.args(["--no-playlist", "--newline", "--no-warnings", "-f", "best[height<=720]/best", "-o"])
        .arg(dir.join("%(title).80s.%(ext)s"))
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(ff) = &tools.ffmpeg {
        if let Some(parent) = ff.parent() {
            cmd.arg("--ffmpeg-location").arg(parent);
        }
    }
    cmd.spawn().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// playback
// ---------------------------------------------------------------------------

struct Pipeline {
    video: Child,
    audio: Option<Child>,
    fps: u32,
    pw: usize,
    ph: usize,
}

impl Pipeline {
    fn stop(&mut self) {
        let _ = self.video.kill();
        let _ = self.video.wait();
        if let Some(a) = self.audio.as_mut() {
            let _ = a.kill();
            let _ = a.wait();
        }
    }
}

fn start_pipeline(src: &str, tools: &Tools, start: f64, mode: ImageMode, cols: usize, rows: usize, fps: u32) -> Result<Pipeline, String> {
    let ffmpeg = tools.ffmpeg.as_ref().ok_or("ffmpeg not found")?;
    let (pw, ph) = ascii::pixel_grid(mode, cols, rows);
    let (pw, ph) = (pw - pw % 2, ph - ph % 4); // keep even for codecs, divisible for cells
    let vf = format!(
        "fps={fps},scale={pw}:{ph}:force_original_aspect_ratio=decrease,pad={pw}:{ph}:(ow-iw)/2:(oh-ih)/2"
    );
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-loglevel", "quiet", "-nostdin"]);
    if start > 0.5 {
        cmd.args(["-ss", &format!("{start:.2}")]);
    }
    cmd.args(["-i", src, "-an", "-vf", &vf, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
    let video = cmd.spawn().map_err(|e| format!("could not start ffmpeg: {e}"))?;
    let audio = tools.ffplay.as_ref().and_then(|p| {
        let mut c = Command::new(p);
        c.args(["-nodisp", "-autoexit", "-loglevel", "quiet", "-vn", "-nostats"]);
        if start > 0.5 {
            c.args(["-ss", &format!("{start:.2}")]);
        }
        c.arg(src).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()
    });
    Ok(Pipeline { video, audio, fps, pw, ph })
}

pub enum Outcome {
    Finished,
    Quit,
}

fn draw(out: &mut impl Write, lines: &[String], pw_cols: usize, term_cols: u16, term_rows: u16, status: &str) {
    let x0 = (term_cols as usize).saturating_sub(pw_cols) / 2;
    let mut s = String::with_capacity(lines.iter().map(|l| l.len() + 12).sum::<usize>() + 256);
    s.push_str("\x1b[?2026h");
    for (i, l) in lines.iter().enumerate() {
        use std::fmt::Write as _;
        let _ = write!(s, "\x1b[{};{}H{}", i + 1, x0 + 1, l);
    }
    use std::fmt::Write as _;
    let _ = write!(s, "\x1b[{};1H\x1b[0m\x1b[2K\x1b[7m{}\x1b[0m", term_rows, util::truncate_width(status, term_cols as usize));
    s.push_str("\x1b[?2026l");
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0) as u64;
    format!("{:02}:{:02}", t / 60, t % 60)
}

/// Blocking full-screen playback. The terminal must already be in raw/alternate-screen mode.
pub fn play(url: &str, title: &str, cfg: &Config, fetcher: &Fetcher, out: &mut impl Write) -> Result<Outcome, String> {
    let tools = tools();
    let is_gif = url.split(['?', '#']).next().map_or(false, |p| p.to_lowercase().ends_with(".gif"));
    if tools.ffmpeg.is_none() || is_gif {
        return play_animated_image(url, cfg, fetcher, out, tools.ffmpeg.is_none());
    }
    let src = resolve_stream(url, &tools)?;
    let mut mode = cfg.image_mode;
    let mut color = cfg.color;
    let fps = cfg.video_fps.clamp(2, 30);
    let mut offset = 0.0f64;
    let audio_note = if tools.ffplay.is_some() { "sound on" } else { "no ffplay: silent" };

    'restart: loop {
        let (tc, tr) = crossterm::terminal::size().map_err(|e| e.to_string())?;
        let (cols, rows) = (tc as usize, (tr as usize).saturating_sub(1).max(4));
        let mut pipe = start_pipeline(&src, &tools, offset, mode, cols, rows, fps)?;
        let frame_bytes = pipe.pw * pipe.ph * 3;
        let mut stdout = pipe.video.stdout.take().ok_or("no video pipe")?;
        let mut buf = vec![0u8; frame_bytes];
        let t0 = Instant::now();
        let mut n: u64 = 0;
        let mut paused = false;
        let frame_dt = Duration::from_secs_f64(1.0 / pipe.fps as f64);
        loop {
            // input
            while event::poll(Duration::from_millis(0)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(KeyEvent { code, modifiers, kind, .. })) if kind != KeyEventKind::Release => {
                        let now_t = offset + n as f64 / pipe.fps as f64;
                        match code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                pipe.stop();
                                return Ok(Outcome::Quit);
                            }
                            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                                pipe.stop();
                                return Ok(Outcome::Quit);
                            }
                            KeyCode::Char(' ') => {
                                pipe.stop();
                                offset = now_t;
                                paused = true;
                                draw(out, &[], 0, tc, tr, &format!(" ⏸ {}  paused — space to resume, q to close", fmt_time(now_t)));
                                loop {
                                    if let Ok(Event::Key(k)) = event::read() {
                                        if k.kind == KeyEventKind::Release {
                                            continue;
                                        }
                                        match k.code {
                                            KeyCode::Char(' ') => break,
                                            KeyCode::Esc | KeyCode::Char('q') => return Ok(Outcome::Quit),
                                            _ => {}
                                        }
                                    }
                                }
                                continue 'restart;
                            }
                            KeyCode::Left => {
                                pipe.stop();
                                offset = (now_t - 10.0).max(0.0);
                                continue 'restart;
                            }
                            KeyCode::Right => {
                                pipe.stop();
                                offset = now_t + 10.0;
                                continue 'restart;
                            }
                            KeyCode::Char('m') => {
                                pipe.stop();
                                offset = now_t;
                                mode = mode.next();
                                continue 'restart;
                            }
                            KeyCode::Char('c') => {
                                color = !color;
                            }
                            _ => {}
                        }
                    }
                    Ok(Event::Resize(..)) => {
                        let now_t = offset + n as f64 / pipe.fps as f64;
                        pipe.stop();
                        offset = now_t;
                        continue 'restart;
                    }
                    _ => {}
                }
            }
            let _ = paused;
            // frame
            if stdout.read_exact(&mut buf).is_err() {
                pipe.stop();
                return Ok(Outcome::Finished);
            }
            let due = t0 + frame_dt * (n as u32);
            let now = Instant::now();
            if now > due + frame_dt * 2 {
                n += 1; // running late: drop this frame
                continue;
            }
            if now < due {
                std::thread::sleep(due - now);
            }
            let lines = ascii::convert(&buf, pipe.pw, pipe.ph, mode, color);
            let t = offset + n as f64 / pipe.fps as f64;
            let status = format!(
                " ▶ {}  {}  [{}{}] {} │ space pause  ←/→ seek  m mode  c color  q close",
                fmt_time(t),
                util::truncate_width(title, 40),
                mode.name(),
                if color { "+color" } else { "" },
                audio_note
            );
            draw(out, &lines, pipe.pw / match mode { ImageMode::Ascii => 1, ImageMode::Blocks => 1, ImageMode::Braille => 2 }, tc, tr, &status);
            n += 1;
        }
    }
}

/// Animated image (GIF) playback using only the compiled-in decoders — no sound.
fn play_animated_image(url: &str, cfg: &Config, fetcher: &Fetcher, out: &mut impl Write, no_ffmpeg: bool) -> Result<Outcome, String> {
    use image::AnimationDecoder;
    let u = Url::parse(url).map_err(|e| e.to_string())?;
    let is_gif = url.split(['?', '#']).next().map_or(false, |p| p.to_lowercase().ends_with(".gif"));
    if !is_gif {
        return Err("Video playback needs ffmpeg (not found). Install ffmpeg, or re-run the Minima installer with --with-media. Animated GIFs play without it.".into());
    }
    let resp = fetcher.get(&u)?;
    let dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(resp.body)).map_err(|e| e.to_string())?;
    let frames: Vec<image::Frame> = dec.into_frames().take(400).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    if frames.is_empty() {
        return Err("empty GIF".into());
    }
    let mut mode = cfg.image_mode;
    let mut color = cfg.color;
    let (mut tc, mut tr) = crossterm::terminal::size().map_err(|e| e.to_string())?;
    let mut cache: Vec<Option<Vec<String>>> = vec![None; frames.len()];
    let mut i = 0;
    loop {
        let f = &frames[i];
        if cache[i].is_none() {
            let img = image::DynamicImage::ImageRgba8(f.buffer().clone());
            cache[i] = Some(ascii::render_image(&img, mode, color, tc as usize, (tr as usize).saturating_sub(1)));
        }
        let lines = cache[i].as_ref().unwrap();
        let cw = lines.first().map(|_| ascii::fit_cells(mode, f.buffer().width(), f.buffer().height(), tc as usize, (tr as usize).saturating_sub(1)).0).unwrap_or(0);
        let status = format!(
            " ▶ GIF {}/{}  [{}{}]{} │ m mode  c color  q close",
            i + 1,
            frames.len(),
            mode.name(),
            if color { "+color" } else { "" },
            if no_ffmpeg { "  (install ffmpeg for video + sound)" } else { "" }
        );
        draw(out, lines, cw, tc, tr, &status);
        let (num, den) = f.delay().numer_denom_ms();
        let delay = Duration::from_millis((num as u64 / den.max(1) as u64).clamp(20, 1000));
        if event::poll(delay).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                    KeyCode::Esc | KeyCode::Char('q') => return Ok(Outcome::Quit),
                    KeyCode::Char('m') => {
                        mode = mode.next();
                        cache.iter_mut().for_each(|c| *c = None);
                    }
                    KeyCode::Char('c') => {
                        color = !color;
                        cache.iter_mut().for_each(|c| *c = None);
                    }
                    _ => {}
                },
                Ok(Event::Resize(w, h)) => {
                    tc = w;
                    tr = h;
                    cache.iter_mut().for_each(|c| *c = None);
                }
                _ => {}
            }
        }
        i = (i + 1) % frames.len();
    }
}
