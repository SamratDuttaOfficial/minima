//! HTML → stripped `Doc`.
//!
//! Two passes over the DOM:
//!  1. bottom-up statistics (words, link density, sentences, widgets...)
//!  2. top-down emission, where the compiled model (`nlp`) decides which
//!     containers are content and which are chrome, which forms matter, and
//!     what every input / button is *for*.

use std::collections::HashMap;

use ego_tree::{NodeId, NodeRef};
use scraper::node::Node;
use scraper::Html;
use url::Url;

use crate::doc::*;
use crate::nlp::{self, ContainerInfo, FormSignals, Stats};
use crate::util;

const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "canvas", "head", "link", "meta", "object", "embed", "map",
    "audio", "datalist", "dialog", "picture-source", "math",
];

const BLOCK_TAGS: &[&str] = &[
    "div", "section", "article", "main", "nav", "aside", "header", "footer", "ul", "ol", "dl", "menu", "table", "form",
    "fieldset", "figure", "details", "center",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Strict,
    Lenient,
}

pub fn extract(url: &Url, html: &str) -> Doc {
    let parsed = Html::parse_document(html);
    let mut doc = build(url, &parsed, html.len(), Mode::Strict);
    // Safety net: if the model stripped nearly everything, redo it leniently.
    let total_words = count_words_total(&parsed);
    let kept_words = doc_words(&doc);
    if total_words > 250 && kept_words * 100 < total_words * 12 {
        let lenient = build(url, &parsed, html.len(), Mode::Lenient);
        if doc_words(&lenient) > kept_words {
            doc = lenient;
        }
    }
    if doc_words(&doc) < 6 && doc.widgets.is_empty() {
        js_wall_fallback(&mut doc, &parsed);
    }
    doc
}

fn doc_words(d: &Doc) -> usize {
    d.blocks.iter().map(|b| d.block_text(b).split_whitespace().count()).sum()
}

fn count_words_total(h: &Html) -> usize {
    let mut n = 0;
    for node in h.tree.nodes() {
        if let Node::Text(t) = node.value() {
            if !in_skipped(node) {
                n += t.split_whitespace().count();
            }
        }
    }
    n
}

fn in_skipped(node: NodeRef<Node>) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        if let Node::Element(e) = p.value() {
            if SKIP_TAGS.contains(&e.name()) {
                return true;
            }
        }
        cur = p.parent();
    }
    false
}

fn js_wall_fallback(doc: &mut Doc, h: &Html) {
    let mut desc = String::new();
    for node in h.tree.nodes() {
        if let Node::Element(e) = node.value() {
            if e.name() == "meta" {
                let key = e.attr("property").or_else(|| e.attr("name")).unwrap_or("");
                if matches!(key, "og:description" | "description" | "twitter:description") {
                    if let Some(c) = e.attr("content") {
                        if c.len() > desc.len() {
                            desc = c.trim().to_string();
                        }
                    }
                }
            }
        }
    }
    // <noscript> content is raw text to the parser; re-parse it as a fragment.
    let mut noscript = String::new();
    for node in h.tree.nodes() {
        if let Node::Element(e) = node.value() {
            if e.name() == "noscript" {
                for c in node.children() {
                    if let Node::Text(t) = c.value() {
                        noscript.push_str(t);
                        noscript.push(' ');
                    }
                }
            }
        }
    }
    if !noscript.trim().is_empty() {
        let frag = Html::parse_fragment(&noscript);
        let sub = build(&doc.url, &frag, noscript.len(), Mode::Lenient);
        if doc_words(&sub) > 3 {
            let base = doc.blocks.len();
            let wbase = doc.widgets.len();
            let fbase = doc.forms.len();
            for mut b in sub.blocks {
                remap_block(&mut b, wbase);
                doc.blocks.push(b);
            }
            for mut w in sub.widgets {
                remap_widget(&mut w, fbase);
                doc.widgets.push(w);
            }
            doc.forms.extend(sub.forms);
            let _ = base;
        }
    }
    doc.blocks.insert(
        0,
        Block::Quote {
            depth: 1,
            inl: vec![Inline::Text(
                "This page builds itself with JavaScript, which Minima does not run. Showing what the page exposes without it.".into(),
            )],
        },
    );
    if !desc.is_empty() {
        doc.blocks.push(Block::Para(vec![Inline::Text(desc)]));
    }
}

fn remap_inl(inl: &mut [Inline], wbase: usize) {
    for i in inl.iter_mut() {
        if let Inline::Widget(w) = i {
            *w += wbase;
        }
    }
}

fn remap_block(b: &mut Block, wbase: usize) {
    match b {
        Block::Heading { inl, .. } | Block::Para(inl) | Block::Item { inl, .. } | Block::Quote { inl, .. } => remap_inl(inl, wbase),
        Block::Row(cells) => cells.iter_mut().for_each(|c| remap_inl(c, wbase)),
        _ => {}
    }
}

fn remap_widget(w: &mut Widget, fbase: usize) {
    let f = match &mut w.kind {
        WidgetKind::Text { form, .. } | WidgetKind::Check { form, .. } | WidgetKind::Select { form, .. } | WidgetKind::Button { form, .. } => form,
        _ => return,
    };
    if let Some(x) = f {
        *x += fbase;
    }
}

fn build(url: &Url, html: &Html, raw_len: usize, mode: Mode) -> Doc {
    let mut b = Builder {
        doc: Doc::empty(url.clone()),
        base: url.clone(),
        stats: HashMap::new(),
        labels: HashMap::new(),
        cur: Vec::new(),
        pending: Pending::Para,
        quote_depth: 0,
        lists: Vec::new(),
        forms: Vec::new(),
        mode,
        stripped: 0,
        page_stats: Stats::default(),
        has_article: false,
        salvaged: Vec::new(),
    };
    let root = html.root_element();
    // <base href>
    for node in html.tree.nodes() {
        if let Node::Element(e) = node.value() {
            if e.name() == "base" {
                if let Some(h) = e.attr("href") {
                    if let Ok(nb) = url.join(h) {
                        b.base = nb;
                    }
                }
                break;
            }
        }
    }
    // title
    for node in html.tree.nodes() {
        if let Node::Element(e) = node.value() {
            if e.name() == "title" {
                let t = collapse(&text_of(node));
                if !t.is_empty() {
                    b.doc.title = t;
                    break;
                }
            }
        }
    }
    // labels for form fields
    for node in html.tree.nodes() {
        if let Node::Element(e) = node.value() {
            if e.name() == "label" {
                if let Some(f) = e.attr("for") {
                    let t = collapse(&text_of(node));
                    if !t.is_empty() {
                        b.labels.insert(f.to_string(), t);
                    }
                }
            }
        }
    }
    // pass 1
    b.page_stats = b.analyze(*root, 0);
    // pass 2
    b.walk_children(*root, 0);
    b.flush();

    if b.doc.title.is_empty() {
        b.doc.title = b
            .doc
            .blocks
            .iter()
            .find_map(|bl| match bl {
                Block::Heading { inl, .. } => Some(b.doc.inline_text(inl)),
                _ => None,
            })
            .unwrap_or_else(|| url.host_str().unwrap_or("").to_string());
    }
    let sig = nlp::PageSignals {
        stats: b.page_stats,
        has_article: b.has_article,
        forms: b.doc.forms.len(),
        url_path: url.path().to_string(),
        url_query: url.query().unwrap_or("").to_string(),
    };
    let salvaged = b.salvaged.clone();
    dedupe_search_forms(&mut b.doc, &salvaged);
    b.doc.kind = nlp::classify_page(&sig);
    b.doc.stripped = b.stripped;
    b.doc.raw_bytes = raw_len;
    b.doc
}

// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Pending {
    Para,
    Heading(u8),
    Item { depth: u8, marker: String },
    Quote(u8),
}

struct ListCtx {
    ordered: bool,
    n: usize,
}

struct Builder {
    doc: Doc,
    base: Url,
    stats: HashMap<NodeId, Stats>,
    labels: HashMap<String, String>,
    cur: Vec<Inline>,
    pending: Pending,
    quote_depth: u8,
    lists: Vec<ListCtx>,
    forms: Vec<Option<usize>>,
    mode: Mode,
    stripped: usize,
    page_stats: Stats,
    has_article: bool,
    salvaged: Vec<usize>,
}

fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut sp = true;
    for ch in s.chars() {
        if ch.is_whitespace() || ch == '\u{a0}' || ch == '\u{200b}' {
            if !sp {
                out.push(' ');
                sp = true;
            }
        } else {
            out.push(ch);
            sp = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// All text under a node (skipping scripts), with image alts.
fn text_of(node: NodeRef<Node>) -> String {
    let mut s = String::new();
    text_rec(node, &mut s);
    s
}

fn text_rec(node: NodeRef<Node>, out: &mut String) {
    match node.value() {
        Node::Text(t) => out.push_str(t),
        Node::Element(e) => {
            let n = e.name();
            if SKIP_TAGS.contains(&n) || is_hidden(e) {
                return;
            }
            if n == "img" {
                if let Some(a) = e.attr("alt") {
                    out.push(' ');
                    out.push_str(a);
                    out.push(' ');
                }
                return;
            }
            if n == "br" {
                out.push(' ');
            }
            for c in node.children() {
                text_rec(c, out);
            }
            if matches!(n, "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                out.push(' ');
            }
        }
        _ => {
            for c in node.children() {
                text_rec(c, out);
            }
        }
    }
}

fn is_hidden(e: &scraper::node::Element) -> bool {
    if e.attr("hidden").is_some() {
        return true;
    }
    if e.attr("aria-hidden") == Some("true") && !matches!(e.name(), "input" | "button") {
        // aria-hidden decorative content (icons etc.)
        return true;
    }
    if let Some(style) = e.attr("style") {
        let s: String = style.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
        if s.contains("display:none") || s.contains("visibility:hidden") {
            return true;
        }
    }
    if e.name() == "input" && e.attr("type").map_or(false, |t| t.eq_ignore_ascii_case("hidden")) {
        return false; // handled as form data
    }
    false
}

fn count_sentences(t: &str) -> u32 {
    let b = t.as_bytes();
    let mut n = 0;
    for i in 0..b.len() {
        if matches!(b[i], b'.' | b'!' | b'?') && (i + 1 == b.len() || b[i + 1] == b' ') {
            n += 1;
        }
    }
    n
}

fn class_id(e: &scraper::node::Element) -> String {
    let mut s = String::new();
    if let Some(c) = e.attr("class") {
        s.push_str(c);
        s.push(' ');
    }
    if let Some(i) = e.attr("id") {
        s.push_str(i);
    }
    s
}

impl Builder {
    // ---- pass 1 ---------------------------------------------------------
    fn analyze(&mut self, node: NodeRef<Node>, depth: u32) -> Stats {
        let mut st = Stats::default();
        match node.value() {
            Node::Text(t) => {
                st.words = t.split_whitespace().count() as u32;
                st.sentences = count_sentences(t);
                return st;
            }
            Node::Element(e) => {
                let n = e.name();
                if SKIP_TAGS.contains(&n) || is_hidden(e) {
                    return st;
                }
                match n {
                    "input" => {
                        let ty = e.attr("type").unwrap_or("text").to_ascii_lowercase();
                        if ty != "hidden" {
                            st.widgets = 1;
                            if !matches!(ty.as_str(), "submit" | "button" | "reset" | "image") {
                                st.inputs = 1;
                            }
                            if ty == "password" {
                                st.password = true;
                            }
                        }
                        return st;
                    }
                    "select" | "textarea" => {
                        st.widgets = 1;
                        st.inputs = 1;
                        return st;
                    }
                    "button" => {
                        st.widgets = 1;
                        st.words = collapse(&text_of(node)).split_whitespace().count() as u32;
                        return st;
                    }
                    _ => {}
                }
                for c in node.children() {
                    let cs = self.analyze(c, depth + 1);
                    st.add(&cs);
                }
                match n {
                    "p" => st.paragraphs += 1,
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => st.headings += 1,
                    "li" => st.items += 1,
                    "a" => {
                        if e.attr("href").is_some() {
                            st.links += 1;
                            st.link_words = st.words;
                        }
                    }
                    _ => {}
                }
                st.depth = depth;
                self.stats.insert(node.id(), st);
            }
            _ => {
                for c in node.children() {
                    let cs = self.analyze(c, depth + 1);
                    st.add(&cs);
                }
            }
        }
        st
    }

    // ---- pass 2 ---------------------------------------------------------
    fn flush(&mut self) {
        let mut inl = std::mem::take(&mut self.cur);
        // trim trailing breaks / spaces
        while matches!(inl.last(), Some(Inline::Break)) {
            inl.pop();
        }
        if let Some(Inline::Text(t)) = inl.last_mut() {
            while t.ends_with(' ') {
                t.pop();
            }
        }
        inl.retain(|i| !matches!(i, Inline::Text(t) if t.is_empty()));
        let pending = std::mem::replace(&mut self.pending, Pending::Para);
        if inl.is_empty() {
            return;
        }
        let blk = match pending {
            Pending::Heading(level) => Block::Heading { level, inl },
            Pending::Item { depth, marker } => Block::Item { depth, marker, inl },
            Pending::Quote(depth) => Block::Quote { depth, inl },
            Pending::Para => {
                if self.quote_depth > 0 {
                    Block::Quote { depth: self.quote_depth, inl }
                } else {
                    Block::Para(inl)
                }
            }
        };
        self.doc.blocks.push(blk);
    }

    fn push_text(&mut self, s: &str) {
        let c = collapse(s);
        if c.is_empty() {
            // pure whitespace: a single separating space
            if s.chars().any(|ch| ch.is_whitespace()) {
                match self.cur.last_mut() {
                    Some(Inline::Text(t)) if !t.ends_with(' ') => t.push(' '),
                    Some(Inline::Widget(_)) => self.cur.push(Inline::Text(" ".into())),
                    _ => {}
                }
            }
            return;
        }
        let lead = s.starts_with(|ch: char| ch.is_whitespace());
        let trail = s.ends_with(|ch: char| ch.is_whitespace());
        let mut text = String::new();
        let need_space = lead
            && match self.cur.last() {
                Some(Inline::Text(t)) => !t.ends_with(' '),
                Some(Inline::Widget(_)) => true,
                _ => false,
            };
        if need_space {
            text.push(' ');
        }
        text.push_str(&c);
        if trail {
            text.push(' ');
        }
        match self.cur.last_mut() {
            Some(Inline::Text(t)) => t.push_str(&text),
            _ => self.cur.push(Inline::Text(text)),
        }
    }

    fn push_widget(&mut self, w: Widget) -> usize {
        // space between adjacent inline things
        if let Some(Inline::Text(t)) = self.cur.last_mut() {
            if !t.ends_with(' ') {
                t.push(' ');
            }
        }
        let id = self.doc.push_widget(w);
        self.cur.push(Inline::Widget(id));
        self.cur.push(Inline::Text(" ".into()));
        id
    }

    fn resolve(&self, href: &str) -> Option<String> {
        let h = href.trim();
        if h.is_empty() || h.starts_with("javascript:") || h.starts_with("data:") {
            return None;
        }
        if h.starts_with('#') {
            return None;
        }
        let u = self.base.join(h).ok()?;
        Some(util::unwrap_redirect(u.as_str()))
    }

    fn walk_children(&mut self, node: NodeRef<Node>, depth: u32) {
        for c in node.children() {
            self.walk(c, depth);
        }
    }

    fn should_drop(&mut self, node: NodeRef<Node>, e: &scraper::node::Element, depth: u32) -> bool {
        let n = e.name();
        let role = e.attr("role").unwrap_or("");
        let st = match self.stats.get(&node.id()) {
            Some(s) => *s,
            None => return false,
        };
        if matches!(n, "html" | "body") {
            return false;
        }
        let cid = class_id(e);
        let semantic_noise = matches!(n, "nav" | "aside" | "footer" | "header" | "dialog")
            || matches!(role, "navigation" | "banner" | "complementary" | "contentinfo" | "dialog" | "alertdialog" | "tooltip");
        let toks = nlp::attr_tokens(&cid);
        let neg_hits = nlp::neg_hits(&toks);
        if self.mode == Mode::Lenient {
            // only obvious chrome
            return (semantic_noise && !matches!(n, "header") && st.link_density() > 0.6 && st.words < 120)
                || (neg_hits > 0 && st.words < 25 && st.link_density() > 0.7);
        }
        // Small plain containers inherit their parent's verdict.
        if st.words < 10 && st.widgets == 0 && !semantic_noise && neg_hits == 0 {
            return false;
        }
        // A container wrapping most of the page's text is the page, not chrome.
        if self.page_stats.words > 0 && st.words * 10 >= self.page_stats.words * 7 && !semantic_noise {
            return false;
        }
        // Search boxes in headers are valuable — keep forms that matter.
        let has_search_form = st.inputs > 0 && st.inputs <= 2 && st.words < 40;
        if has_search_form && neg_hits == 0 && st.link_density() < 0.6 {
            return false;
        }
        let info = ContainerInfo { tag: n, role, class_id: &cid, stats: st };
        let z = nlp::score_container(&info);
        let _ = depth;
        if std::env::var("MINIMA_DEBUG").is_ok() { eprintln!("container <{}> cls={:?} z={:.2} words={} widgets={} inputs={}", n, cid, z, st.words, st.widgets, st.inputs); }
        z < nlp::container_threshold()
    }

    fn walk(&mut self, node: NodeRef<Node>, depth: u32) {
        match node.value() {
            Node::Text(t) => {
                // text inside <pre> is handled by the pre branch
                self.push_text(t);
            }
            Node::Element(e) => {
                let n = e.name();
                if SKIP_TAGS.contains(&n) {
                    return;
                }
                if is_hidden(e) {
                    return;
                }
                if BLOCK_TAGS.contains(&n) || n == "body" {
                    if n != "form" && self.forms.is_empty() && self.should_drop(node, e, depth) {
                        self.stripped += 1;
                        self.salvage_search(node, depth);
                        return;
                    }
                }
                self.element(node, e, n, depth);
            }
            _ => self.walk_children(node, depth),
        }
    }


    /// A dropped container (header, nav...) may still hold the page's search box. Keep that.
    fn salvage_search(&mut self, node: NodeRef<Node>, depth: u32) {
        for d in node.descendants() {
            if let Node::Element(e) = d.value() {
                if e.name() != "form" || is_hidden(e) {
                    continue;
                }
                let Some(st) = self.stats.get(&d.id()).copied() else { continue };
                if st.inputs == 0 {
                    continue;
                }
                let mut labels = Vec::new();
                form_labels(d, &mut labels);
                let cid = class_id(e);
                let sig = FormSignals { action: e.attr("action").unwrap_or(""), class_id: &cid, labels: &labels, has_text: form_has_textfield(d), fields: (st.inputs + st.widgets) as usize, password: st.password };
                let (z, intent) = nlp::score_form(&sig);
                if intent == "search" && z >= nlp::form_threshold() {
                    self.flush();
                    let fid = self.doc.forms.len();
                    self.form(d, e, depth);
                    if self.doc.forms.len() > fid {
                        self.salvaged.push(fid);
                    }
                    self.flush();
                    return;
                }
            }
        }
    }
    /// Should an empty field show its label inside the box? Not when a visible label already sits next to it.
    fn wants_hint(&self, node: NodeRef<Node>, e: &scraper::node::Element) -> bool {
        if e.attr("placeholder").map_or(false, |p| !p.trim().is_empty()) || e.attr("aria-label").map_or(false, |p| !p.trim().is_empty()) {
            return true;
        }
        let flow = e.attr("id").map_or(false, |id| self.labels.contains_key(id)) || has_label_ancestor(node) || matches!(self.cur.last(), Some(Inline::Text(t)) if !t.trim().is_empty() && t.len() < 40);
        !flow
    }

    fn label_for(&self, node: NodeRef<Node>, e: &scraper::node::Element) -> String {
        if let Some(id) = e.attr("id") {
            if let Some(l) = self.labels.get(id) {
                return l.clone();
            }
        }
        if let Some(a) = e.attr("aria-label").filter(|s| !s.trim().is_empty()) {
            return collapse(a);
        }
        // wrapped in <label>?
        let mut cur = node.parent();
        let mut hops = 0;
        while let Some(p) = cur {
            if let Node::Element(pe) = p.value() {
                if pe.name() == "label" {
                    let t = collapse(&text_of(p));
                    if !t.is_empty() {
                        return t;
                    }
                }
                if pe.name() == "form" {
                    break;
                }
            }
            hops += 1;
            if hops > 4 {
                break;
            }
            cur = p.parent();
        }
        for k in ["placeholder", "title", "alt"] {
            if let Some(a) = e.attr(k).filter(|s| !s.trim().is_empty()) {
                return collapse(a);
            }
        }
        // text just before us in this block
        if let Some(Inline::Text(t)) = self.cur.last() {
            let t = t.trim();
            if !t.is_empty() && t.len() < 40 {
                return t.trim_end_matches(':').to_string();
            }
        }
        let name = e.attr("name").unwrap_or("");
        pretty_name(name)
    }

    fn element(&mut self, node: NodeRef<Node>, e: &scraper::node::Element, n: &str, depth: u32) {
        match n {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let level = n.as_bytes()[1] - b'0';
                self.pending = Pending::Heading(level);
                self.walk_children(node, depth + 1);
                self.flush();
            }
            "p" | "address" | "figcaption" | "summary" | "caption" | "legend" => {
                self.flush();
                self.walk_children(node, depth + 1);
                self.flush();
            }
            "br" => {
                if !self.cur.is_empty() && !matches!(self.cur.last(), Some(Inline::Break)) {
                    self.cur.push(Inline::Break);
                }
            }
            "hr" => {
                self.flush();
                self.doc.blocks.push(Block::Rule);
            }
            "blockquote" => {
                self.flush();
                self.quote_depth += 1;
                self.walk_children(node, depth + 1);
                self.flush();
                self.quote_depth -= 1;
            }
            "pre" => {
                self.flush();
                let mut raw = String::new();
                raw_text(node, &mut raw);
                let raw = raw.trim_matches('\n').trim_end().to_string();
                if !raw.trim().is_empty() {
                    self.doc.blocks.push(Block::Pre(raw));
                }
            }
            "ul" | "ol" | "menu" => {
                self.flush();
                self.lists.push(ListCtx { ordered: n == "ol", n: 0 });
                self.walk_children(node, depth + 1);
                self.flush();
                self.lists.pop();
            }
            "li" => {
                self.flush();
                let d = self.lists.len().saturating_sub(1).min(6) as u8;
                let marker = match self.lists.last_mut() {
                    Some(l) if l.ordered => {
                        l.n += 1;
                        format!("{}.", l.n)
                    }
                    _ => "•".to_string(),
                };
                self.pending = Pending::Item { depth: d, marker };
                self.walk_children(node, depth + 1);
                self.flush();
            }
            "dt" => {
                self.flush();
                self.walk_children(node, depth + 1);
                self.flush();
            }
            "dd" => {
                self.flush();
                self.pending = Pending::Item { depth: 1, marker: "–".into() };
                self.walk_children(node, depth + 1);
                self.flush();
            }
            "table" => {
                self.flush();
                self.walk_children(node, depth + 1);
                self.flush();
            }
            "tr" => {
                self.flush();
                let mut cells: Vec<Vec<Inline>> = Vec::new();
                for c in node.children() {
                    if let Node::Element(ce) = c.value() {
                        if matches!(ce.name(), "td" | "th") && !is_hidden(ce) {
                            let saved = std::mem::take(&mut self.cur);
                            self.walk_children(c, depth + 2);
                            let mut cell = std::mem::replace(&mut self.cur, saved);
                            while matches!(cell.last(), Some(Inline::Break)) {
                                cell.pop();
                            }
                            if let Some(Inline::Text(t)) = cell.last_mut() {
                                while t.ends_with(' ') {
                                    t.pop();
                                }
                            }
                            if let Some(Inline::Text(t)) = cell.first_mut() {
                                *t = t.trim_start().to_string();
                            }
                            cell.retain(|i| !matches!(i, Inline::Text(t) if t.is_empty()));
                            if !cell.is_empty() {
                                cells.push(cell);
                            }
                        }
                    }
                }
                if cells.len() == 1 {
                    self.cur = cells.pop().unwrap();
                    self.flush();
                } else if !cells.is_empty() {
                    self.doc.blocks.push(Block::Row(cells));
                }
            }
            "a" => self.anchor(node, e),
            "img" => self.image(e),
            "video" => {
                let src = e
                    .attr("src")
                    .map(|s| s.to_string())
                    .or_else(|| {
                        node.children().find_map(|c| match c.value() {
                            Node::Element(se) if se.name() == "source" => se.attr("src").map(|s| s.to_string()),
                            _ => None,
                        })
                    });
                if let Some(src) = src.and_then(|s| self.resolve(&s)) {
                    let label = e.attr("title").or_else(|| e.attr("aria-label")).map(collapse).unwrap_or_else(|| "video".into());
                    self.flush();
                    self.push_widget(Widget::new(WidgetKind::Video { src }, label));
                    self.flush();
                }
            }
            "iframe" => {
                if let Some(src) = e.attr("src").and_then(|s| self.resolve(&s)) {
                    let lower = src.to_lowercase();
                    let video = lower.contains("youtube.com/embed") || lower.contains("youtube-nocookie.com/embed") || lower.contains("player.vimeo.com") || lower.contains("dailymotion.com/embed");
                    if video {
                        let watch = crate::media::embed_to_watch(&src);
                        let label = e.attr("title").map(collapse).unwrap_or_else(|| "embedded video".into());
                        self.flush();
                        self.push_widget(Widget::new(WidgetKind::Video { src: watch }, label));
                        self.flush();
                    }
                }
            }
            "form" => self.form(node, e, depth),
            "input" => self.input(node, e),
            "textarea" => {
                let name = e.attr("name").unwrap_or("").to_string();
                let value = collapse_keep_lines(&text_of_raw(node));
                let label = self.label_for(node, e);
                let form = self.forms.last().cloned().flatten();
                self.push_widget(Widget::new(WidgetKind::Text { form, name, value, password: false, multiline: true }, label));
            }
            "select" => {
                let mut options = Vec::new();
                let mut selected = 0;
                collect_options(node, &mut options, &mut selected);
                if options.is_empty() {
                    return;
                }
                if let Some(f) = self.forms.last().cloned().flatten() {
                    if self.doc.forms[f].intent == "search" {
                        // filters in a search box are noise: keep their default as a hidden value
                        let nm = e.attr("name").unwrap_or("").to_string();
                        if !nm.is_empty() {
                            if let Some(o) = options.get(selected) {
                                self.doc.forms[f].hidden.push((nm, o.0.clone()));
                            }
                        }
                        return;
                    }
                }
                let name = e.attr("name").unwrap_or("").to_string();
                let label = self.label_for(node, e);
                let form = self.forms.last().cloned().flatten();
                self.push_widget(Widget::new(WidgetKind::Select { form, name, options, selected }, label));
            }
            "button" => {
                let ty = e.attr("type").unwrap_or("submit").to_ascii_lowercase();
                let mut label = collapse(&text_of(node));
                if label.is_empty() {
                    label = e
                        .attr("aria-label")
                        .or_else(|| e.attr("title"))
                        .or_else(|| e.attr("value"))
                        .map(collapse)
                        .unwrap_or_default();
                }
                if label.is_empty() {
                    label = "button".into();
                }
                let form = self.forms.last().cloned().flatten();
                let submit = ty == "submit" && form.is_some();
                if form.is_none() && self.mode == Mode::Strict && ty != "submit" {
                    // JS-only buttons are useless here, except to show they exist
                    self.push_text(&format!("[{}] ", label));
                    return;
                }
                let name = e.attr("name").unwrap_or("").to_string();
                let value = e.attr("value").unwrap_or("").to_string();
                self.push_widget(Widget::new(WidgetKind::Button { form, name, value, submit }, label));
            }
            "label" | "span" | "b" | "strong" | "i" | "em" | "u" | "small" | "mark" | "abbr" | "cite" | "code" | "kbd" | "samp" | "sub" | "sup" | "time" | "font" | "q" | "s" | "tt" | "big" | "var" | "bdi" | "bdo" | "data" | "ins" | "del" => {
                self.walk_children(node, depth + 1);
            }
            "details" => {
                self.flush();
                self.walk_children(node, depth + 1);
                self.flush();
            }
            "option" | "optgroup" | "source" | "track" | "param" | "area" | "col" | "colgroup" | "wbr" => {}
            _ => {
                // div, section, td, body, unknown elements...
                let block_like = BLOCK_TAGS.contains(&n) || matches!(n, "td" | "th" | "tbody" | "thead" | "tfoot" | "body" | "html" | "dl");
                if matches!(n, "article" | "main") || e.attr("role") == Some("main") {
                    self.has_article = true;
                }
                if block_like {
                    self.flush();
                    self.walk_children(node, depth + 1);
                    self.flush();
                } else {
                    self.walk_children(node, depth + 1);
                }
            }
        }
    }

    fn anchor(&mut self, node: NodeRef<Node>, e: &scraper::node::Element) {
        let href = e.attr("href").and_then(|h| self.resolve(h));
        let href = match href {
            Some(h) => h,
            None => {
                let t = collapse(&text_of(node)).to_lowercase();
                if t.starts_with("jump to") || t.starts_with("skip to") || t.starts_with("skip navigation") {
                    return;
                }
                self.walk_children(node, 1);
                return;
            }
        };
        let plain = collapse(&text_of_noalt(node));
        // block-ish anchors (cards) break the line before/after
        let block_anchor = node.children().any(|c| matches!(c.value(), Node::Element(ce) if matches!(ce.name(), "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "section" | "article" | "figure")));
        let heading = node.descendants().find_map(|d| match d.value() {
            Node::Element(he) if matches!(he.name(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                let t = collapse(&text_of_noalt(d));
                if t.is_empty() { None } else { Some(t) }
            }
            _ => None,
        });
        // first real image inside the anchor (card thumbnails, image-only links)
        let first_img = node.descendants().find_map(|d| match d.value() {
            Node::Element(ie) if ie.name() == "img" => Some(ie.clone()),
            _ => None,
        });

        if plain.is_empty() {
            // image-only link: expose the picture (viewable as ASCII art)
            if let Some(img) = first_img {
                let before = self.doc.widgets.len();
                self.image(&img);
                if self.doc.widgets.len() > before {
                    return;
                }
            }
            let label = e.attr("aria-label").or_else(|| e.attr("title")).map(collapse).unwrap_or_default();
            if label.is_empty() {
                return;
            }
            self.push_widget(Widget::new(WidgetKind::Link { href }, label));
            return;
        }
        {
            let l = plain.to_lowercase();
            if l.starts_with("jump to ") || l.starts_with("skip to ") || l == "skip navigation" || l == "skip to content" {
                return;
            }
        }
        let (mut label, rest) = match (&heading, block_anchor) {
            (Some(h), true) if h.len() < plain.len() => {
                let rest = collapse(&plain.replacen(h.as_str(), " ", 1));
                (h.clone(), rest)
            }
            _ => (plain.clone(), String::new()),
        };
        if label.chars().count() > 180 {
            label = label.chars().take(177).collect::<String>() + "…";
        }
        if block_anchor {
            self.flush();
            // card thumbnail first
            if let Some(img) = &first_img {
                self.image(img);
                self.flush();
            }
        }
        self.push_widget(Widget::new(WidgetKind::Link { href }, label));
        if block_anchor {
            self.flush();
            if !rest.is_empty() {
                let mut r = rest;
                if r.chars().count() > 240 {
                    r = r.chars().take(237).collect::<String>() + "…";
                }
                self.cur.push(Inline::Text(r));
                self.flush();
            }
        }
    }

    fn image(&mut self, e: &scraper::node::Element) {
        let src = e
            .attr("src")
            .filter(|s| !s.starts_with("data:") || s.len() > 400)
            .or_else(|| e.attr("data-src"))
            .or_else(|| e.attr("data-original"))
            .or_else(|| e.attr("data-lazy-src"))
            .or_else(|| e.attr("srcset").and_then(|s| s.split(',').next()).and_then(|s| s.split_whitespace().next()));
        let src = match src.and_then(|s| self.resolve(s)) {
            Some(s) => s,
            None => return,
        };
        let dim = |k: &str| e.attr(k).and_then(|v| v.trim_end_matches("px").parse::<u32>().ok());
        let (w, h) = (dim("width"), dim("height"));
        if matches!((w, h), (Some(a), _) if a <= 48) || matches!((w, h), (_, Some(b)) if b <= 48) {
            return;
        }
        let alt = e.attr("alt").map(collapse).unwrap_or_default();
        if alt.is_empty() && w.is_none() && h.is_none() {
            let cid = class_id(e).to_lowercase();
            if cid.contains("icon") || cid.contains("logo") || cid.contains("sprite") || cid.contains("pixel") || cid.contains("emoji") {
                return;
            }
        }
        let label = if alt.is_empty() {
            Url::parse(&src).ok().map(|u| util::filename_from_url(&u)).filter(|s| s != "download" && !s.is_empty()).unwrap_or_else(|| "image".to_string())
        } else {
            alt
        };
        self.push_widget(Widget::new(WidgetKind::Image { src }, label));
    }

    fn input(&mut self, node: NodeRef<Node>, e: &scraper::node::Element) {
        if e.attr("name").map_or(false, |n| n.to_lowercase().contains("hidden")) || e.attr("class").map_or(false, |c| c.to_lowercase().split_whitespace().any(|t| t.contains("hidden") || t == "sr-only" || t == "visually-hidden")) {
            if e.attr("type").map_or(true, |t| !t.eq_ignore_ascii_case("hidden")) {
                return;
            }
        }
        let ty = e.attr("type").unwrap_or("text").to_ascii_lowercase();
        let name = e.attr("name").unwrap_or("").to_string();
        let value = e.attr("value").unwrap_or("").to_string();
        let form = self.forms.last().cloned().flatten();
        match ty.as_str() {
            "hidden" => {
                if let (Some(f), false) = (form, name.is_empty()) {
                    self.doc.forms[f].hidden.push((name, value));
                }
            }
            "submit" | "button" | "reset" | "image" => {
                if ty == "reset" {
                    return;
                }
                let mut label = if ty == "image" { e.attr("alt").unwrap_or("") } else { &value }.to_string();
                if label.is_empty() {
                    label = e.attr("aria-label").or_else(|| e.attr("title")).unwrap_or("").to_string();
                }
                if label.is_empty() {
                    label = if ty == "submit" || ty == "image" { "Submit".into() } else { "button".into() };
                }
                let submit = ty != "button" && form.is_some();
                if !submit && form.is_none() && self.mode == Mode::Strict {
                    return;
                }
                self.push_widget(Widget::new(WidgetKind::Button { form, name, value, submit }, collapse(&label)));
            }
            "checkbox" | "radio" => {
                let radio = ty == "radio";
                let checked = e.attr("checked").is_some();
                let value = if value.is_empty() { "on".to_string() } else { value };
                let enclosed = self.label_for(node, e);
                let flow_label = self.labels.contains_key(e.attr("id").unwrap_or("\u{0}"))
                    || has_label_ancestor(node);
                let id = self.push_widget(Widget::new(WidgetKind::Check { form, name, value, checked, radio }, enclosed.clone()));
                let _ = id;
                if !flow_label && !enclosed.is_empty() {
                    self.push_text(&enclosed);
                }
            }
            "file" | "color" | "range" => {
                let label = self.label_for(node, e);
                self.push_text(&format!("[{} {}] ", ty, label));
            }
            _ => {
                let hint = self.wants_hint(node, e);
                let label = self.label_for(node, e);
                let password = ty == "password";
                let id = self.push_widget(Widget::new(WidgetKind::Text { form, name, value, password, multiline: false }, label));
                self.doc.widgets[id].hint = hint;
            }
        }
    }

    fn form(&mut self, node: NodeRef<Node>, e: &scraper::node::Element, depth: u32) {
        let st = self.stats.get(&node.id()).copied().unwrap_or_default();
        let mut labels = Vec::new();
        form_labels(node, &mut labels);
        let cid = class_id(e);
        let action = e.attr("action").unwrap_or("");
        let sig = FormSignals { action, class_id: &cid, labels: &labels, has_text: form_has_textfield(node), fields: (st.inputs + st.widgets) as usize, password: st.password };
        let (z, intent) = nlp::score_form(&sig);
        if std::env::var("MINIMA_DEBUG").is_ok() { eprintln!("form action={:?} z={} widgets={} inputs={} intent={}", action, z, st.widgets, st.inputs, intent); }
        if st.widgets == 0 || (self.mode == Mode::Strict && z < nlp::form_threshold() && st.inputs < 3 && self.page_stats.words >= 150) {
            if st.widgets > 0 {
                self.stripped += 1;
            } else {
                self.flush();
                self.walk_children(node, depth + 1);
                self.flush();
            }
            return;
        }
        let action_url = if action.trim().is_empty() {
            self.doc.url.to_string()
        } else {
            self.base.join(action.trim()).map(|u| u.to_string()).unwrap_or_else(|_| self.doc.url.to_string())
        };
        let method = if e.attr("method").map_or(false, |m| m.eq_ignore_ascii_case("post")) { Method::Post } else { Method::Get };
        let id = self.doc.forms.len();
        self.doc.forms.push(FormInfo { action: action_url, method, hidden: Vec::new(), intent: intent.to_string() });
        self.flush();
        self.forms.push(Some(id));
        self.walk_children(node, depth + 1);
        self.flush();
        self.forms.pop();
    }
}

fn has_label_ancestor(node: NodeRef<Node>) -> bool {
    let mut cur = node.parent();
    let mut hops = 0;
    while let Some(p) = cur {
        if let Node::Element(pe) = p.value() {
            if pe.name() == "label" {
                return true;
            }
        }
        hops += 1;
        if hops > 3 {
            break;
        }
        cur = p.parent();
    }
    false
}

fn pretty_name(name: &str) -> String {
    let t: Vec<String> = nlp::attr_tokens(name);
    if t.is_empty() {
        return String::new();
    }
    let mut s = t.join(" ");
    if let Some(f) = s.get(0..1) {
        let up = f.to_uppercase();
        s.replace_range(0..1, &up);
    }
    s
}

fn form_labels(node: NodeRef<Node>, out: &mut Vec<String>) {
    for d in node.descendants() {
        if let Node::Element(e) = d.value() {
            match e.name() {
                "input" | "textarea" | "select" => {
                    for k in ["placeholder", "aria-label", "name", "title", "type"] {
                        if let Some(v) = e.attr(k) {
                            if !v.is_empty() {
                                out.push(v.to_string());
                            }
                        }
                    }
                    if e.name() == "input" && matches!(e.attr("type"), Some("submit") | Some("button")) {
                        if let Some(v) = e.attr("value") {
                            out.push(v.to_string());
                        }
                    }
                }
                "button" => out.push(collapse(&text_of(d))),
                "label" => out.push(collapse(&text_of(d))),
                _ => {}
            }
        }
    }
}

fn collect_options(node: NodeRef<Node>, out: &mut Vec<(String, String)>, selected: &mut usize) {
    for c in node.children() {
        if let Node::Element(e) = c.value() {
            match e.name() {
                "option" => {
                    let text = collapse(&text_of(c));
                    let value = e.attr("value").map(|s| s.to_string()).unwrap_or_else(|| text.clone());
                    if e.attr("selected").is_some() {
                        *selected = out.len();
                    }
                    out.push((value, if text.is_empty() { "(blank)".into() } else { text }));
                }
                "optgroup" => collect_options(c, out, selected),
                _ => {}
            }
        }
    }
}

fn raw_text(node: NodeRef<Node>, out: &mut String) {
    for c in node.children() {
        match c.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => {
                if e.name() == "br" {
                    out.push('\n');
                } else if !SKIP_TAGS.contains(&e.name()) {
                    raw_text(c, out);
                }
            }
            _ => {}
        }
    }
}

fn text_of_raw(node: NodeRef<Node>) -> String {
    let mut s = String::new();
    raw_text(node, &mut s);
    s
}

fn collapse_keep_lines(s: &str) -> String {
    s.trim_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(html: &str) -> Doc {
        extract(&Url::parse("https://example.com/").unwrap(), html)
    }

    #[test]
    fn strips_nav_and_keeps_article() {
        let html = r#"<html><head><title>T</title></head><body>
        <nav class="menu"><a href="/a">Home</a><a href="/b">Products</a><a href="/c">Pricing</a><a href="/d">Blog</a><a href="/e">Login</a></nav>
        <article><h1>Hello world</h1><p>This is a real paragraph of content. It has sentences. And more of them for good measure, so that the model is confident.</p>
        <p>Second paragraph, also quite long, with more text that makes this clearly the main content of the page. Really.</p></article>
        <footer><a href="/t">Terms</a> <a href="/p">Privacy</a> © 2025</footer></body></html>"#;
        let d = run(html);
        let all: String = d.blocks.iter().map(|b| d.block_text(b)).collect::<Vec<_>>().join("\n");
        assert!(all.contains("Hello world"));
        assert!(all.contains("real paragraph"));
        assert!(!all.contains("Pricing"), "nav leaked: {all}");
        assert!(!all.contains("Privacy"), "footer leaked: {all}");
        assert_eq!(d.title, "T");
    }

    #[test]
    fn search_form_found() {
        let html = r#"<html><body><form action="/search" method="get"><input name="q" placeholder="Search the web"><input type="submit" value="Go"></form></body></html>"#;
        let d = run(html);
        assert_eq!(d.forms.len(), 1);
        assert_eq!(d.forms[0].intent, "search");
        assert!(d.widgets.iter().any(|w| w.is_text() && w.label == "Search the web"));
    }

    #[test]
    fn google_style_search_box_in_table_is_kept() {
        let html = r#"<html><body><center><img alt="Logo" src="/l.png" width="272" height="92"><form action="/search"><table><tr><td width="25%">&nbsp;</td><td><input name="hl" type="hidden" value="en"><div class="ds" style="height:32px"><input class="lst" title="Google Search" name="q" size="57"></div><br><span class="ds"><input class="lsb" value="Google Search" name="btnG" type="submit"></span></td></tr></table></form></center></body></html>"#;
        let d = run(html);
        assert!(d.widgets.iter().any(|w| w.is_text()), "search box missing: {:?}", d.widgets);
        assert!(d.widgets.iter().any(|w| matches!(w.kind, WidgetKind::Button { .. })));
    }

    #[test]
    fn plain_order_form_survives_even_with_comments_field() {
        let html = r#"<html><body><form method="post" action="/post"><p><label>Customer name: <input name="custname"></label></p><p><label>Telephone: <input name="custtel" type="tel"></label></p><p><label>E-mail: <input type="email" name="custemail"></label></p><p><label>Delivery instructions: <textarea name="comments"></textarea></label></p><p><button>Submit order</button></p></form></body></html>"#;
        let d = run(html);
        assert!(d.widgets.iter().filter(|w| w.is_text()).count() >= 4);
    }

    #[test]
    fn search_box_inside_header_is_salvaged() {
        let html = r#"<html><body><header class="site-header"><nav><a href="/a">A</a><a href="/b">B</a><a href="/c">C</a><a href="/d">D</a></nav><form action="/w/index.php" role="search"><input type="search" name="search" placeholder="Search Wikipedia"><button>Search</button></form></header><main><article><h1>T</h1><p>Some long article text here. It has sentences. And more of them, enough to be content for sure, really.</p></article></main></body></html>"#;
        let d = run(html);
        assert!(d.widgets.iter().any(|w| w.is_text() && w.label == "Search Wikipedia"), "{:?}", d.widgets);
    }

    #[test]
    fn card_link_uses_heading_as_label() {
        let html = r#"<html><body><main><p>Some intro text that is long enough to be content. More sentences follow here. And here.</p><a href="/story/1"><div><img src="/t.jpg" alt="A photo caption that is long" width="300" height="200"><h3>Big news today</h3><p>Summary text of the story goes here.</p></div></a></main></body></html>"#;
        let d = run(html);
        let link = d.widgets.iter().find(|w| matches!(w.kind, WidgetKind::Link { .. })).unwrap();
        assert_eq!(link.label, "Big news today");
        assert!(d.widgets.iter().any(|w| matches!(w.kind, WidgetKind::Image { .. })));
        let all: String = d.blocks.iter().map(|b| d.block_text(b)).collect::<Vec<_>>().join("|");
        assert!(all.contains("Summary text"));
    }

    #[test]
    fn links_resolved() {
        let d = run(r#"<body><main><p>Some long text here, to be sure that this is content. More text follows. And more.</p><a href="/x/y">Link</a></main></body>"#);
        assert!(d.widgets.iter().any(|w| matches!(&w.kind, WidgetKind::Link { href } if href == "https://example.com/x/y")));
    }
}
fn text_of_noalt(node: NodeRef<Node>) -> String {
    let mut s = String::new();
    noalt_rec(node, &mut s);
    s
}

fn noalt_rec(node: NodeRef<Node>, out: &mut String) {
    match node.value() {
        Node::Text(t) => out.push_str(t),
        Node::Element(e) => {
            let n = e.name();
            if SKIP_TAGS.contains(&n) || is_hidden(e) || n == "img" {
                return;
            }
            if n == "br" {
                out.push(' ');
            }
            for c in node.children() {
                noalt_rec(c, out);
            }
            if matches!(n, "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "figcaption") {
                out.push(' ');
            }
        }
        _ => {
            for c in node.children() {
                noalt_rec(c, out);
            }
        }
    }
}

/// Pages like Wikipedia's results show their own search box as well as the header one we
/// salvaged. Keep only the last search form per action (the earlier widgets become unreferenced).
fn dedupe_search_forms(doc: &mut Doc, salvaged: &[usize]) {
    // which widgets are actually shown?
    let mut shown = vec![false; doc.widgets.len()];
    let mark = |inl: &[Inline], shown: &mut Vec<bool>| {
        for i in inl {
            if let Inline::Widget(w) = i {
                shown[*w] = true;
            }
        }
    };
    for b in &doc.blocks {
        match b {
            Block::Heading { inl, .. } | Block::Para(inl) | Block::Item { inl, .. } | Block::Quote { inl, .. } => mark(inl, &mut shown),
            Block::Row(cells) => cells.iter().for_each(|c| mark(c, &mut shown)),
            _ => {}
        }
    }
    let form_of = |w: &Widget| match &w.kind {
        WidgetKind::Text { form: Some(f), .. } | WidgetKind::Button { form: Some(f), .. } | WidgetKind::Check { form: Some(f), .. } | WidgetKind::Select { form: Some(f), .. } => Some(*f),
        _ => None,
    };
    let mut visible_forms: Vec<usize> = Vec::new();
    for (i, w) in doc.widgets.iter().enumerate() {
        if shown[i] {
            if let Some(f) = form_of(w) {
                if doc.forms[f].intent == "search" && !visible_forms.contains(&f) {
                    visible_forms.push(f);
                }
            }
        }
    }
    let mut drop_forms: Vec<usize> = Vec::new();
    for &f in &visible_forms {
        let same: Vec<usize> = visible_forms.iter().cloned().filter(|&g| doc.forms[g].action == doc.forms[f].action).collect();
        if same.len() < 2 {
            continue;
        }
        // keep the last form that was part of the page itself; if all were salvaged, keep the first
        let keep = same.iter().cloned().filter(|g| !salvaged.contains(g)).last().unwrap_or(same[0]);
        if f != keep {
            drop_forms.push(f);
        }
    }
    if std::env::var("MINIMA_DEBUG").is_ok() { eprintln!("forms={:?} visible={:?} drop={:?}", doc.forms.iter().map(|f| (f.intent.clone(), f.action.clone())).collect::<Vec<_>>(), visible_forms, drop_forms); }
    if drop_forms.is_empty() {
        return;
    }
    let dropped: Vec<bool> = doc.widgets.iter().map(|w| form_of(w).map_or(false, |f| drop_forms.contains(&f))).collect();
    let strip = |inl: &mut Vec<Inline>| inl.retain(|i| !matches!(i, Inline::Widget(w) if dropped[*w]));
    for b in doc.blocks.iter_mut() {
        match b {
            Block::Heading { inl, .. } | Block::Para(inl) | Block::Item { inl, .. } | Block::Quote { inl, .. } => strip(inl),
            Block::Row(cells) => cells.iter_mut().for_each(|c| strip(c)),
            _ => {}
        }
    }
    doc.blocks.retain(|b| match b {
        Block::Para(inl) => inl.iter().any(|i| match i {
            Inline::Text(t) => !t.trim().is_empty(),
            _ => true,
        }),
        _ => true,
    });
}

/// Does this form contain a real text-entry control (not just selects and buttons)?
fn form_has_textfield(node: NodeRef<Node>) -> bool {
    node.descendants().any(|d| match d.value() {
        Node::Element(e) => match e.name() {
            "textarea" => true,
            "input" => !matches!(
                e.attr("type").unwrap_or("text").to_ascii_lowercase().as_str(),
                "hidden" | "submit" | "button" | "reset" | "image" | "checkbox" | "radio" | "file" | "color" | "range"
            ),
            _ => false,
        },
        _ => false,
    })
}
