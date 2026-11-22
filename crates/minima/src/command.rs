//! The “ask Minima” command bar: a small intent parser on top of the compiled
//! synonym-aware matcher. No network, no cloud model.

use crate::app::App;
use crate::doc::*;
use crate::nlp::Matcher;
use crate::util;

fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        let rest = &s[prefix.len()..];
        // must end at a word boundary
        if rest.is_empty() || rest.starts_with(' ') || prefix.ends_with(' ') {
            return Some(rest.trim());
        }
    }
    None
}

fn first_prefix<'a>(s: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|p| strip_ci(s, p))
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    for (a, b) in [('"', '"'), ('\'', '\''), ('“', '”')] {
        if t.len() >= 2 && t.starts_with(a) && t.ends_with(b) {
            return t[a.len_utf8()..t.len() - b.len_utf8()].to_string();
        }
    }
    t.to_string()
}

pub fn run(app: &mut App, text: &str) {
    let mut raw = text.trim();
    if raw.is_empty() {
        return;
    }
    for p in ["please ", "can you ", "could you ", "go ahead and ", "i want to ", "i'd like to ", "let's ", "just "] {
        if let Some(r) = strip_ci(raw, p) {
            raw = r;
        }
    }
    let raw = raw.trim_end_matches(['.', '!']).trim();
    let lower = raw.to_lowercase();

    match lower.as_str() {
        "back" | "go back" | "previous page" => return app.go_back(),
        "forward" | "go forward" | "next page" => return app.go_forward(),
        "reload" | "refresh" | "reload page" => return app.reload(),
        "home" | "go home" | "home page" => return app.navigate("about:home", None),
        "history" | "show history" | "open history" => return app.show_history(),
        "bookmarks" | "show bookmarks" | "open bookmarks" | "favorites" => return app.show_bookmarks(),
        "bookmark" | "bookmark this" | "bookmark page" | "bookmark this page" | "star" => return app.toggle_bookmark(),
        "help" | "?" | "what can you do" | "commands" => return app.navigate("about:help", None),
        "quit" | "exit" | "close" | "bye" | "quit minima" => {
            app.quit = true;
            return;
        }
        "links" | "show links" | "list links" | "controls" => return app.show_links(),
        "images" | "show images" | "list images" | "pictures" | "show pictures" => return app.show_images(),
        "top" | "scroll to top" | "go to top" | "to the top" => {
            app.scroll = 0;
            return app.say("top");
        }
        "bottom" | "end" | "scroll to bottom" | "go to bottom" | "to the bottom" => {
            app.scroll_to_line(usize::MAX / 4);
            return;
        }
        "scroll down" | "page down" => {
            let h = app.size.1 as isize - 3;
            return app.scroll_by(h);
        }
        "scroll up" | "page up" => {
            let h = app.size.1 as isize - 3;
            return app.scroll_by(-h);
        }
        "download" | "download this" | "download it" | "download page" | "save" | "save this" => {
            let f = app.focus;
            return app.download_target(f);
        }
        "play" | "play video" | "play the video" | "watch" | "watch video" | "play it" => {
            return play_matching(app, "");
        }
        "submit" | "press enter" | "send" | "submit form" | "go" => return app.submit_focused_or_first_form(),
        _ => {}
    }

    if let Some(rest) = first_prefix(raw, &["go to ", "goto ", "open ", "visit ", "navigate to ", "browse to ", "take me to ", "load "]) {
        if !rest.is_empty() {
            let r = unquote(rest);
            // "open the login page" → treat as a click if something on the page matches
            let looks_like_url = !r.contains(' ') && (r.contains('.') || r.contains("://") || r.starts_with("localhost"));
            if !looks_like_url && click(app, &r, true) {
                return;
            }
            return app.open_input(&r);
        }
    }
    if let Some(rest) = first_prefix(raw, &["search for ", "search ", "google ", "look up ", "lookup ", "find me ", "web search "]) {
        return do_search(app, &unquote(rest));
    }
    if let Some(rest) = first_prefix(raw, &["find ", "locate "]) {
        return app.do_find(&unquote(rest));
    }
    if let Some(rest) = first_prefix(raw, &["view image ", "show image ", "look at image ", "open image ", "see image ", "image "]) {
        return view_image(app, rest);
    }
    if let Some(rest) = first_prefix(raw, &["play ", "watch "]) {
        return play_matching(app, rest);
    }
    if let Some(rest) = first_prefix(raw, &["download "]) {
        return download_matching(app, rest);
    }
    if let Some(rest) = first_prefix(raw, &["click on ", "click ", "press ", "tap ", "follow ", "hit ", "select ", "choose ", "push ", "activate ", "go with "]) {
        if !click(app, &unquote(rest), false) {
            app.err(format!("nothing on this page matches “{}”", unquote(rest)));
        }
        return;
    }
    if let Some(rest) = first_prefix(raw, &["fill in ", "fill out ", "fill ", "set "]) {
        // "fill FIELD with VALUE" / "set FIELD to VALUE"
        for sep in [" with ", " to ", " as ", " = "] {
            if let Some(i) = rest.find(sep) {
                let (field, value) = (rest[..i].trim(), rest[i + sep.len()..].trim());
                return fill(app, &unquote(field), &unquote(value), true);
            }
        }
        return fill(app, "", &unquote(rest), true);
    }
    if let Some(rest) = first_prefix(raw, &["type ", "enter ", "write ", "input ", "put "]) {
        for sep in [" into the ", " in the ", " into ", " in ", " on the ", " on ", " to "] {
            if let Some(i) = rest.to_lowercase().rfind(sep) {
                let (value, field) = (rest[..i].trim(), rest[i + sep.len()..].trim());
                let field = field.trim_end_matches(" field").trim_end_matches(" box").trim_end_matches(" input");
                return fill(app, &unquote(field), &unquote(value), true);
            }
        }
        return fill(app, "", &unquote(rest), true);
    }

    // Anything else: treat as a question about this page.
    ask_page(app, raw);
}

fn text_widgets(doc: &Doc) -> Vec<usize> {
    doc.widgets.iter().enumerate().filter(|(_, w)| w.is_text()).map(|(i, _)| i).collect()
}

pub fn fill(app: &mut App, field: &str, value: &str, quiet_submit_hint: bool) {
    let Some(doc) = app.doc() else { return };
    let texts = text_widgets(doc);
    if texts.is_empty() {
        return app.err("there are no text fields on this page");
    }
    let target = if field.is_empty() {
        app.focus
            .filter(|f| texts.contains(f))
            .or_else(|| {
                texts.iter().cloned().find(|i| {
                    matches!(&doc.widgets[*i].kind, WidgetKind::Text { form: Some(f), .. } if doc.forms[*f].intent == "search")
                })
            })
            .or_else(|| texts.first().cloned())
    } else {
        let m = Matcher::new(field);
        let mut best: Option<(usize, f32)> = None;
        for &i in &texts {
            let w = &doc.widgets[i];
            let name = if let WidgetKind::Text { name, .. } = &w.kind { name.clone() } else { String::new() };
            let s = m.score_label(&format!("{} {}", w.label, name));
            if s > 0.0 && best.map_or(true, |b| s > b.1) {
                best = Some((i, s));
            }
        }
        best.map(|b| b.0)
    };
    let Some(t) = target else {
        return app.err(format!("no field matches “{field}”"));
    };
    let label = doc.widgets[t].label.clone();
    if let Some(d) = app.doc_mut() {
        if let WidgetKind::Text { value: v, .. } = &mut d.widgets[t].kind {
            *v = value.to_string();
        }
        d.widgets[t].cursor = value.chars().count();
    }
    app.relayout();
    app.set_focus(Some(t));
    if quiet_submit_hint {
        app.say(format!("filled “{}” — Enter on the field submits, or say “submit”", label));
    }
}

fn do_search(app: &mut App, q: &str) {
    if q.is_empty() {
        return;
    }
    let target = app.doc().and_then(|d| {
        d.widgets.iter().enumerate().find_map(|(i, w)| match &w.kind {
            WidgetKind::Text { form: Some(f), .. } if d.forms[*f].intent == "search" => Some((i, *f)),
            _ => None,
        })
    });
    match target {
        Some((w, f)) => {
            if let Some(d) = app.doc_mut() {
                if let WidgetKind::Text { value, .. } = &mut d.widgets[w].kind {
                    *value = q.to_string();
                }
            }
            app.submit_form(Some(f), None);
        }
        None => {
            let u = util::search_url(&app.cfg.search.clone(), q);
            app.navigate(&u, None);
        }
    }
}

/// Click the best-matching control. Returns true if something matched.
fn click(app: &mut App, query: &str, quiet: bool) -> bool {
    let Some(doc) = app.doc() else { return false };
    let m = Matcher::new(query);
    if m.is_empty() {
        return false;
    }
    let mut best: Option<(usize, f32)> = None;
    for (i, w) in doc.widgets.iter().enumerate() {
        let extra = match &w.kind {
            WidgetKind::Link { href } => {
                // a link's URL path can carry meaning (/login, /pricing)
                url::Url::parse(href).ok().map(|u| u.path().replace(['/', '-', '_'], " ")).unwrap_or_default()
            }
            _ => String::new(),
        };
        let mut s = m.score_label(&w.label);
        if s == 0.0 && !extra.trim().is_empty() {
            s = m.score_label(&extra) * 0.6;
        }
        if matches!(w.kind, WidgetKind::Button { submit: true, .. } | WidgetKind::Link { .. }) {
            s *= 1.1;
        }
        if s > 0.0 && best.map_or(true, |b| s > b.1) {
            best = Some((i, s));
        }
    }
    match best {
        Some((i, s)) if s >= 0.6 => {
            let label = doc.widgets[i].label.clone();
            app.set_focus(Some(i));
            if !quiet {
                app.say(format!("clicking “{}”", label));
            }
            app.activate(i);
            true
        }
        _ => false,
    }
}

fn view_image(app: &mut App, rest: &str) {
    let Some(doc) = app.doc() else { return };
    let imgs: Vec<usize> = doc.widgets.iter().enumerate().filter(|(_, w)| matches!(w.kind, WidgetKind::Image { .. })).map(|(i, _)| i).collect();
    if imgs.is_empty() {
        return app.err("no images on this page");
    }
    let rest = rest.trim();
    let pick = if let Ok(n) = rest.parse::<usize>() {
        imgs.get(n.saturating_sub(1)).cloned()
    } else if rest.is_empty() {
        imgs.first().cloned()
    } else {
        let m = Matcher::new(rest);
        imgs.iter().cloned().map(|i| (i, m.score(&doc.widgets[i].label))).filter(|x| x.1 > 0.0).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|x| x.0)
    };
    match pick {
        Some(w) => {
            app.set_focus(Some(w));
            app.activate(w);
        }
        None => app.err("no such image"),
    }
}

fn play_matching(app: &mut App, rest: &str) {
    let Some(doc) = app.doc() else { return };
    let vids: Vec<usize> = doc.widgets.iter().enumerate().filter(|(_, w)| matches!(w.kind, WidgetKind::Video { .. })).map(|(i, _)| i).collect();
    let pick = if rest.trim().is_empty() || rest.trim() == "video" || rest.trim() == "the video" {
        vids.first().cloned()
    } else {
        // maybe a link to a video page: "play <title>"
        let m = Matcher::new(rest);
        doc.widgets
            .iter()
            .enumerate()
            .filter(|(_, w)| matches!(w.kind, WidgetKind::Video { .. } | WidgetKind::Link { .. }))
            .map(|(i, w)| (i, m.score_label(&w.label)))
            .filter(|x| x.1 > 0.0)
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|x| x.0)
    };
    match pick {
        Some(w) => {
            app.set_focus(Some(w));
            app.activate(w);
        }
        None => app.err("no video found on this page"),
    }
}

fn download_matching(app: &mut App, rest: &str) {
    let Some(doc) = app.doc() else { return };
    let r = rest.trim();
    if r.is_empty() || r == "this" || r == "it" || r == "page" {
        let f = app.focus;
        return app.download_target(f);
    }
    let m = Matcher::new(r);
    let pick = doc
        .widgets
        .iter()
        .enumerate()
        .filter(|(_, w)| matches!(w.kind, WidgetKind::Link { .. } | WidgetKind::Image { .. } | WidgetKind::Video { .. }))
        .map(|(i, w)| (i, m.score(&w.label)))
        .filter(|x| x.1 > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|x| x.0);
    match pick {
        Some(w) => app.download_target(Some(w)),
        None => app.err(format!("nothing to download matching “{r}”")),
    }
}

/// Jump to the block that best answers a free-form question.
fn ask_page(app: &mut App, q: &str) {
    let Some(doc) = app.doc() else { return };
    let m = Matcher::new(q);
    let mut best: Option<(usize, f32)> = None;
    for (bi, b) in doc.blocks.iter().enumerate() {
        let t = doc.block_text(b);
        let mut s = m.score(&t);
        if matches!(b, Block::Heading { .. }) {
            s *= 1.4;
        }
        if s > 0.0 && best.map_or(true, |x| s > x.1) {
            best = Some((bi, s));
        }
    }
    let mut widget_best: Option<(usize, f32)> = None;
    for (i, w) in doc.widgets.iter().enumerate() {
        let s = m.score_label(&w.label);
        if s > 0.0 && widget_best.map_or(true, |x| s > x.1) {
            widget_best = Some((i, s));
        }
    }
    // a strongly matching control wins ("login" → the login link)
    if let Some((w, s)) = widget_best {
        if s >= best.map_or(0.0, |b| b.1) * 0.9 && s >= 1.0 {
            let label = doc.widgets[w].label.clone();
            app.set_focus(Some(w));
            return app.say(format!("selected “{}” — press Enter to use it", label));
        }
    }
    if let Some((bi, s)) = best {
        if s >= 0.8 {
            let snippet = util::truncate_width(&doc.block_text(&doc.blocks[bi]), 70);
            let line = app.layout_ref().and_then(|l| l.block_starts.get(bi).cloned());
            if let Some(line) = line {
                app.scroll_to_line(line);
                return app.say(format!("best match: “{snippet}”"));
            }
        }
    }
    // nothing relevant here: treat it as a web search or URL
    app.say(format!("nothing on this page matches — searching the web for “{}”", q));
    app.open_input(q);
}
