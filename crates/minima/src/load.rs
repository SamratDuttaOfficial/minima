//! Background page loading: fetch → classify content → `Doc`.

use image::DynamicImage;
use url::Url;

use crate::doc::*;
use crate::extract;
use crate::fetch::{decode_html, Fetched, Fetcher};
use crate::youtube;

pub enum Loaded {
    Doc(Doc),
    Image(DynamicImage, Url),
    Video(Url),
    Binary(Url, String),
}

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const VIDEO_EXT: &[&str] = &["mp4", "webm", "mkv", "mov", "avi", "m3u8", "m4v", "ogv", "flv", "mp3", "ogg", "wav", "m4a", "flac", "opus"];
const BINARY_EXT: &[&str] = &[
    "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "tar", "exe", "msi", "dmg", "pkg", "deb", "rpm", "appimage", "iso", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "apk", "jar", "bin",
];

fn ext_of(u: &Url) -> String {
    u.path_segments()
        .and_then(|s| s.last())
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

pub fn load(fetcher: &Fetcher, url: &Url, post: Option<&[(String, String)]>, search_template: &str) -> Result<Loaded, String> {
    let _ = search_template;
    if post.is_none() && youtube::handles(url) {
        if let Ok(d) = youtube::build(fetcher, url) {
            return Ok(Loaded::Doc(d));
        }
    }
    let ext = ext_of(url);
    if post.is_none() {
        if VIDEO_EXT.contains(&ext.as_str()) {
            return Ok(Loaded::Video(url.clone()));
        }
        if BINARY_EXT.contains(&ext.as_str()) {
            return Ok(Loaded::Binary(url.clone(), ext));
        }
    }
    let resp = match post {
        Some(fields) => fetcher.post_form(url, fields)?,
        None => fetcher.get(url)?,
    };
    let mut loaded = classify(resp, &ext)?;

    // Google's results page needs JavaScript for many clients; fall back to DuckDuckGo's HTML endpoint.
    if let Loaded::Doc(d) = &mut loaded {
        if is_google_search(url) && looks_blocked(d) {
            if let Some(q) = url.query_pairs().find(|(k, _)| k == "q").map(|(_, v)| v.to_string()) {
                let alt = Url::parse(&format!("https://html.duckduckgo.com/html/?q={}", crate::util::url_encode(&q))).map_err(|e| e.to_string())?;
                if let Ok(r) = fetcher.get(&alt) {
                    if let Ok(Loaded::Doc(mut d2)) = classify(r, "") {
                        d2.blocks.insert(
                            0,
                            Block::Quote {
                                depth: 1,
                                inl: vec![Inline::Text("Google would not serve results without JavaScript, so these come from DuckDuckGo for the same query.".into())],
                            },
                        );
                        d2.title = format!("{q} — search");
                        return Ok(Loaded::Doc(d2));
                    }
                }
            }
        }
    }
    Ok(loaded)
}

fn is_google_search(u: &Url) -> bool {
    u.host_str().map_or(false, |h| h.contains("google.")) && u.path() == "/search"
}

fn looks_blocked(d: &Doc) -> bool {
    let text: String = d.blocks.iter().take(12).map(|b| d.block_text(b)).collect::<Vec<_>>().join(" ").to_lowercase();
    let words = text.split_whitespace().count();
    text.contains("enable javascript")
        || text.contains("unusual traffic")
        || text.contains("not a robot")
        || text.contains("httpservice/retry")
        || d.url.path().starts_with("/sorry")
        || (words < 80 && d.widgets.iter().filter(|w| matches!(w.kind, WidgetKind::Link { .. })).count() < 5)
}

fn classify(resp: Fetched, ext: &str) -> Result<Loaded, String> {
    let ct = resp.content_type.to_lowercase();
    let url = resp.url.clone();
    let ct_main = ct.split(';').next().unwrap_or("").trim().to_string();
    if ct_main.starts_with("image/") && !ct_main.contains("svg") {
        let img = image::load_from_memory(&resp.body).map_err(|e| format!("cannot decode image: {e}"))?;
        return Ok(Loaded::Image(img, url));
    }
    if ct_main.starts_with("image/svg") {
        return Err("SVG images are not supported in the terminal viewer (press d to download)".into());
    }
    if ct_main.starts_with("video/") || ct_main.starts_with("audio/") || ct_main == "application/vnd.apple.mpegurl" || ct_main == "application/x-mpegurl" {
        return Ok(Loaded::Video(url));
    }
    let sniff_html = {
        let head = &resp.body[..resp.body.len().min(512)];
        let h = String::from_utf8_lossy(head).to_lowercase();
        h.contains("<html") || h.contains("<!doctype html") || h.contains("<head") || h.contains("<body")
    };
    let is_html = ct_main.contains("html") || ct_main.contains("xhtml") || (ct_main.is_empty() && sniff_html) || (ct_main == "text/plain" && sniff_html && IMAGE_EXT.iter().all(|e| *e != ext));
    if is_html {
        let text = decode_html(&resp.body, &resp.content_type);
        let mut doc = extract::extract(&url, &text);
        doc.raw_bytes = resp.body.len();
        let words: usize = doc.blocks.iter().map(|b| doc.block_text(b).split_whitespace().count()).sum();
        if resp.status >= 400 && words < 25 {
            let reason = match resp.status {
                400 => "Bad request",
                401 => "Sign-in required",
                403 => "Forbidden: the site refused to show this to Minima",
                404 => "Not found",
                410 => "Gone",
                429 => "Too many requests: slow down and retry later",
                500 => "Server error",
                502 | 503 | 504 => "The site is unavailable right now",
                _ => "The server reported an error",
            };
            doc.blocks = vec![
                Block::Heading { level: 1, inl: vec![Inline::Text(format!("HTTP {}", resp.status))] },
                Block::Para(vec![Inline::Text(reason.to_string())]),
                Block::Para(vec![Inline::Text("Press r to retry, Backspace to go back, or g to go somewhere else.".into())]),
            ];
            doc.widgets.clear();
            doc.forms.clear();
        }
        return Ok(Loaded::Doc(doc));
    }
    if ct_main.starts_with("text/") || ct_main.contains("json") || ct_main.contains("xml") || ct_main.contains("javascript") {
        let mut text = decode_html(&resp.body, &resp.content_type);
        if ct_main.contains("json") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Ok(p) = serde_json::to_string_pretty(&v) {
                    text = p;
                }
            }
        }
        let mut doc = Doc::builder(url.clone(), url.path_segments().and_then(|s| s.last()).unwrap_or("text"));
        if doc.title.is_empty() {
            doc.title = url.host_str().unwrap_or("text").to_string();
        }
        doc.raw_bytes = resp.body.len();
        doc.blocks.push(Block::Pre(text));
        return Ok(Loaded::Doc(doc));
    }
    Ok(Loaded::Binary(url, ct_main))
}
