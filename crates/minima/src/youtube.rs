//! YouTube special-casing: its pages are pure JavaScript, but the data is in
//! the HTML as JSON, so we can build clean result and watch pages ourselves.

use serde_json::Value;
use url::Url;

use crate::doc::*;
use crate::fetch::{decode_html, Fetcher};
use crate::media;
use crate::util;

pub fn handles(u: &Url) -> bool {
    let host = u.host_str().unwrap_or("");
    media::is_youtube_host(host) && (media::youtube_id(u).is_some() || u.path() == "/results")
}

pub fn build(fetcher: &Fetcher, u: &Url) -> Result<Doc, String> {
    if let Some(id) = media::youtube_id(u) {
        return watch_doc(fetcher, &id);
    }
    results_doc(fetcher, u)
}

fn search_form(doc: &mut Doc, q: &str) -> usize {
    doc.forms.push(FormInfo {
        action: "https://www.youtube.com/results".into(),
        method: Method::Get,
        hidden: Vec::new(),
        intent: "search".into(),
    });
    let f = doc.forms.len() - 1;
    let t = doc.push_widget(Widget::new(
        WidgetKind::Text { form: Some(f), name: "search_query".into(), value: q.to_string(), password: false, multiline: false },
        "Search YouTube".into(),
    ));
    let b = doc.push_widget(Widget::new(
        WidgetKind::Button { form: Some(f), name: String::new(), value: String::new(), submit: true },
        "Search".into(),
    ));
    doc.blocks.push(Block::Para(vec![Inline::Widget(t), Inline::Text(" ".into()), Inline::Widget(b)]));
    f
}

fn watch_doc(fetcher: &Fetcher, id: &str) -> Result<Doc, String> {
    let watch = media::watch_url(id);
    let url = Url::parse(&watch).map_err(|e| e.to_string())?;
    let mut doc = Doc::builder(url.clone(), "YouTube");
    doc.kind = PageKind::Article;
    let oembed = format!("https://www.youtube.com/oembed?format=json&url={}", util::url_encode(&watch));
    let (title, author, thumb) = match Url::parse(&oembed).ok().and_then(|u| fetcher.get(&u).ok()) {
        Some(r) if r.status == 200 => {
            let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            (
                v["title"].as_str().unwrap_or("YouTube video").to_string(),
                v["author_name"].as_str().unwrap_or("").to_string(),
                v["thumbnail_url"].as_str().map(|s| s.to_string()),
            )
        }
        _ => ("YouTube video".to_string(), String::new(), None),
    };
    doc.title = title.clone();
    doc.blocks.push(Block::Heading { level: 1, inl: vec![Inline::Text(title.clone())] });
    if !author.is_empty() {
        doc.blocks.push(Block::Para(vec![Inline::Text(format!("by {author}"))]));
    }
    let play = doc.push_widget(Widget::new(WidgetKind::Video { src: watch.clone() }, format!("Play “{}” as ASCII art (p)", util::truncate_width(&title, 50))));
    doc.blocks.push(Block::Para(vec![Inline::Widget(play)]));
    if let Some(t) = thumb {
        let w = doc.push_widget(Widget::new(WidgetKind::Image { src: t }, "thumbnail".into()));
        doc.blocks.push(Block::Para(vec![Inline::Widget(w)]));
    }
    doc.blocks.push(Block::Para(vec![Inline::Text("Press p to play with sound, d to download. Needs yt-dlp and ffmpeg.".into())]));
    doc.blocks.push(Block::Rule);
    search_form(&mut doc, "");
    Ok(doc)
}

fn results_doc(fetcher: &Fetcher, u: &Url) -> Result<Doc, String> {
    let q = u.query_pairs().find(|(k, _)| k == "search_query").map(|(_, v)| v.to_string()).unwrap_or_default();
    let mut fetch_url = u.clone();
    // ask for the "videos only" filter so ytInitialData is mostly videoRenderers
    if !u.query_pairs().any(|(k, _)| k == "sp") {
        fetch_url.query_pairs_mut().append_pair("sp", "EgIQAQ%3D%3D");
    }
    let resp = fetcher.get(&fetch_url)?;
    let html = decode_html(&resp.body, &resp.content_type);
    let json = extract_initial_data(&html).ok_or("YouTube did not return result data")?;
    let v: Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let mut vids = Vec::new();
    collect_videos(&v, &mut vids);
    let mut doc = Doc::builder(u.clone(), &format!("{q} - YouTube"));
    doc.kind = PageKind::Results;
    doc.blocks.push(Block::Heading { level: 1, inl: vec![Inline::Text(format!("YouTube: {q}"))] });
    search_form(&mut doc, &q);
    doc.blocks.push(Block::Rule);
    if vids.is_empty() {
        doc.blocks.push(Block::Para(vec![Inline::Text("No videos found.".into())]));
    }
    for (id, title, meta) in vids.into_iter().take(40) {
        let w = doc.push_widget(Widget::new(WidgetKind::Link { href: media::watch_url(&id) }, title));
        doc.blocks.push(Block::Item {
            depth: 0,
            marker: "▶".into(),
            inl: vec![Inline::Widget(w), Inline::Break, Inline::Text(meta)],
        });
    }
    Ok(doc)
}

fn extract_initial_data(html: &str) -> Option<String> {
    let key = "ytInitialData";
    let mut from = 0;
    while let Some(i) = html[from..].find(key) {
        let start = from + i + key.len();
        let rest = &html[start..];
        let brace = rest.find('{')?;
        // must be an assignment: only `=`/spaces/quotes before the brace
        if rest[..brace].chars().all(|c| matches!(c, ' ' | '=' | '\'' | '"' | '(' | '\\')) {
            let body = &rest[brace..];
            let bytes = body.as_bytes();
            let (mut depth, mut in_str, mut esc) = (0i32, false, false);
            for (k, &b) in bytes.iter().enumerate() {
                if in_str {
                    if esc {
                        esc = false;
                    } else if b == b'\\' {
                        esc = true;
                    } else if b == b'"' {
                        in_str = false;
                    }
                    continue;
                }
                match b {
                    b'"' => in_str = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(body[..=k].to_string());
                        }
                    }
                    _ => {}
                }
            }
            return None;
        }
        from = start;
    }
    None
}

fn text_of(v: &Value) -> String {
    if let Some(s) = v["simpleText"].as_str() {
        return s.to_string();
    }
    if let Some(runs) = v["runs"].as_array() {
        return runs.iter().filter_map(|r| r["text"].as_str()).collect::<Vec<_>>().join("");
    }
    String::new()
}

fn collect_videos(v: &Value, out: &mut Vec<(String, String, String)>) {
    match v {
        Value::Object(m) => {
            if let Some(vr) = m.get("videoRenderer") {
                if let Some(id) = vr["videoId"].as_str() {
                    let title = text_of(&vr["title"]);
                    let parts: Vec<String> = [
                        text_of(&vr["ownerText"]),
                        text_of(&vr["lengthText"]),
                        text_of(&vr["viewCountText"]),
                        text_of(&vr["publishedTimeText"]),
                    ]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect();
                    if !title.is_empty() && !out.iter().any(|o| o.0 == id) {
                        out.push((id.to_string(), title, parts.join("  ·  ")));
                    }
                }
                return;
            }
            for (_, c) in m {
                collect_videos(c, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|c| collect_videos(c, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ids() {
        let u = Url::parse("https://youtu.be/abc123").unwrap();
        assert_eq!(media::youtube_id(&u).as_deref(), Some("abc123"));
        let u = Url::parse("https://www.youtube.com/watch?v=xyz&t=3").unwrap();
        assert_eq!(media::youtube_id(&u).as_deref(), Some("xyz"));
    }

    #[test]
    fn extracts_json() {
        let html = r#"<script>var ytInitialData = {"a":{"videoRenderer":{"videoId":"id1","title":{"runs":[{"text":"Hi {x}"}]}}}};</script>"#;
        let j = extract_initial_data(html).unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        let mut out = Vec::new();
        collect_videos(&v, &mut out);
        assert_eq!(out[0].0, "id1");
        assert_eq!(out[0].1, "Hi {x}");
    }
}
