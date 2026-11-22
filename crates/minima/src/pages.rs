//! Built-in `about:` pages: home, help, history.

use url::Url;

use crate::config::Config;
use crate::doc::*;
use crate::history::Store;

pub fn about(target: &str, cfg: &Config, hist: &Store) -> Option<Doc> {
    match target {
        "about:home" | "about:" => Some(home(cfg)),
        "about:help" => Some(help()),
        "about:history" => Some(history(hist)),
        "about:blank" => Some(Doc::builder(Url::parse("about:blank").ok()?, "blank")),
        _ => None,
    }
}

fn about_url(s: &str) -> Url {
    Url::parse(s).unwrap_or_else(|_| Url::parse("about:blank").unwrap())
}

fn link(doc: &mut Doc, href: &str, label: &str) -> Inline {
    Inline::Widget(doc.push_widget(Widget::new(WidgetKind::Link { href: href.into() }, label.into())))
}

/// Split a `https://host/search?q={q}` template into (action, field name, other params).
pub fn parse_search_template(t: &str) -> (String, String, Vec<(String, String)>) {
    let (base, query) = t.split_once('?').unwrap_or((t, "q={q}"));
    let mut name = "q".to_string();
    let mut hidden = Vec::new();
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if v.contains("{q}") {
                name = k.to_string();
            } else {
                hidden.push((k.to_string(), v.to_string()));
            }
        }
    }
    (base.to_string(), name, hidden)
}

pub fn home(cfg: &Config) -> Doc {
    let mut d = Doc::builder(about_url("about:home"), "Minima");
    d.kind = PageKind::Home;
    d.blocks.push(Block::Heading { level: 1, inl: vec![Inline::Text("Minima".into())] });
    d.blocks.push(Block::Para(vec![Inline::Text("The web, stripped down to what matters — right in your terminal.".into())]));
    let (action, name, hidden) = parse_search_template(&cfg.search);
    d.forms.push(FormInfo { action, method: Method::Get, hidden, intent: "search".into() });
    let t = d.push_widget(Widget::new(
        WidgetKind::Text { form: Some(0), name, value: String::new(), password: false, multiline: false },
        "Search the web".into(),
    ));
    let b = d.push_widget(Widget::new(WidgetKind::Button { form: Some(0), name: String::new(), value: String::new(), submit: true }, "Search".into()));
    d.blocks.push(Block::Para(vec![Inline::Widget(t), Inline::Text(" ".into()), Inline::Widget(b)]));
    d.blocks.push(Block::Heading { level: 2, inl: vec![Inline::Text("Quick links".into())] });
    for (label, href) in [
        ("Wikipedia", "https://en.wikipedia.org/"),
        ("Hacker News", "https://news.ycombinator.com/"),
        ("YouTube (as ASCII art)", "https://www.youtube.com/"),
        ("DuckDuckGo", "https://html.duckduckgo.com/html/"),
        ("Rust docs", "https://doc.rust-lang.org/"),
        ("Google", "https://www.google.com/"),
    ] {
        let l = link(&mut d, href, label);
        d.blocks.push(Block::Item { depth: 0, marker: "›".into(), inl: vec![l] });
    }
    d.blocks.push(Block::Para(vec![Inline::Text("Type to search, g to go to a URL, : to talk to Minima (“click sign in”), ? for help.".into())]));
    d
}

pub fn history(h: &Store) -> Doc {
    let mut d = Doc::builder(about_url("about:history"), "History");
    d.blocks.push(Block::Heading { level: 1, inl: vec![Inline::Text("History".into())] });
    for e in h.recent().into_iter().take(200) {
        let label = if e.title.is_empty() { e.url.clone() } else { e.title.clone() };
        let l = link(&mut d, &e.url, &label);
        d.blocks.push(Block::Item { depth: 0, marker: "›".into(), inl: vec![l] });
    }
    d
}

pub fn help() -> Doc {
    let mut d = Doc::builder(about_url("about:help"), "Minima help");
    let h = |d: &mut Doc, lvl: u8, t: &str| d.blocks.push(Block::Heading { level: lvl, inl: vec![Inline::Text(t.into())] });
    let item = |d: &mut Doc, key: &str, desc: &str| {
        d.blocks.push(Block::Item { depth: 0, marker: "•".into(), inl: vec![Inline::Text(format!("{key} — {desc}"))] })
    };
    h(&mut d, 1, "Minima help");
    d.blocks.push(Block::Para(vec![Inline::Text("Every page is fetched, stripped of ads / menus / banners by a compiled-in content model, and drawn as text. Links, buttons and form fields stay usable.".into())]));
    h(&mut d, 2, "Moving around");
    item(&mut d, "Tab / Shift+Tab", "next / previous link, button or field");
    item(&mut d, "↑ ↓", "move to the link or field above / below (scrolls when there is none)");
    item(&mut d, "← →", "move sideways between controls on the same line; at the edge: back / forward");
    item(&mut d, "Enter", "follow link, press button, edit field, toggle checkbox, open menu");
    item(&mut d, "Space / PgDn, PgUp, Home, End, j / k", "scroll");
    item(&mut d, "Mouse", "wheel scrolls, click activates");
    h(&mut d, 2, "Going places");
    item(&mut d, "g", "go to a URL, or type words to search");
    item(&mut d, "G or Ctrl+L", "edit the current URL");
    item(&mut d, "Backspace / h / Alt+←", "back");
    item(&mut d, "Alt+→", "forward");
    item(&mut d, "r / F5", "reload");
    item(&mut d, "~", "home page");
    item(&mut d, "H", "history  (filter by typing, Del removes)");
    item(&mut d, "b / B", "bookmark this page / bookmark list");
    item(&mut d, "L", "list every link and control on the page, filterable");
    h(&mut d, 2, "Ask Minima (:)");
    d.blocks.push(Block::Para(vec![Inline::Text("Press : and say what you want. The synonym-aware matcher is compiled in; no network, no cloud model.".into())]));
    for ex in [
        "search for rust async book",
        "click sign in          (also: log in, login, account…)",
        "type alice in username",
        "fill email with a@b.com",
        "go to news.ycombinator.com",
        "view image 2    ·    show images",
        "play video      ·    download this",
        "price           (anything else jumps to the best matching text on the page)",
        "back · forward · reload · history · bookmarks · help · quit",
    ] {
        item(&mut d, ":", ex);
    }
    h(&mut d, 2, "Images and video");
    item(&mut d, "i / Enter on an image", "view as art. m cycles ascii / blocks / braille, c toggles colour, - and 0 zoom, ←/→ next image, d saves");
    item(&mut d, "I", "list all images");
    item(&mut d, "p", "play a video (ffmpeg + ffplay for sound; yt-dlp for YouTube). GIFs need nothing extra");
    item(&mut d, "d", "download the focused link, image or video (or the page)");
    h(&mut d, 2, "Finding text");
    item(&mut d, "/ then n / N", "find, next, previous");
    h(&mut d, 2, "Quit");
    item(&mut d, "q or Ctrl+C", "quit");
    d
}
