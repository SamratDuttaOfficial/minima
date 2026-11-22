//! The interactive browser: event loop, navigation, forms, overlays, image viewer.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use image::DynamicImage;
use url::Url;

use crate::ascii;
use crate::command;
use crate::config::{Config, ImageMode};
use crate::doc::*;
use crate::fetch::{self, Fetcher};
use crate::history::Store;
use crate::layout::{self, Layout, Style};
use crate::load::{self, Loaded};
use crate::media;
use crate::pages;
use crate::util::{self, truncate_width, width};

pub enum Msg {
    Loaded { gen: u64, result: Result<Loaded, String> },
    Image { gen: u64, result: Result<DynamicImage, String>, from_widget: Option<usize>, src: String },
    Status(String, bool),
}

struct Page {
    doc: Doc,
    scroll: usize,
    focus: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Url,
    Find,
    Command,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub buf: Vec<char>,
    pub cur: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    History,
    Bookmarks,
    Images,
    Links,
    Options(usize),
}

pub struct ListItem {
    pub label: String,
    pub detail: String,
    pub key: String,
}

pub struct ListView {
    pub kind: ListKind,
    pub title: String,
    pub items: Vec<ListItem>,
    pub sel: usize,
    pub top: usize,
    pub filter: Vec<char>,
}

impl ListView {
    fn visible(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.items.len()).collect();
        }
        let q: String = self.filter.iter().collect();
        let m = crate::nlp::Matcher::new(&q);
        let ql = q.to_lowercase();
        let mut scored: Vec<(usize, f32)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let hay = format!("{} {}", it.label, it.detail);
                let lower = hay.to_lowercase();
                let s = m.score(&hay) + if lower.contains(&ql) { 3.0 } else { 0.0 };
                if s > 0.0 {
                    Some((i, s))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|x| x.0).collect()
    }
}

pub struct ImageView {
    pub img: DynamicImage,
    pub src: String,
    pub mode: ImageMode,
    pub color: bool,
    pub lines: Vec<String>,
    pub cells_w: usize,
    pub zoom: f32,
    pub siblings: Vec<usize>,
    pub idx: usize,
}

pub enum Mode {
    Browse,
    Edit(usize),
    Prompt(Prompt),
    List(ListView),
    Image(ImageView),
}

struct FindState {
    query: String,
    lines: Vec<usize>,
    idx: usize,
}

pub struct App {
    pub cfg: Config,
    pub fetcher: Fetcher,
    pub hist: Store,
    pub marks: Store,
    pages: Vec<Page>,
    pos: usize,
    lay: Option<Layout>,
    lay_w: usize,
    pub focus: Option<usize>,
    pub scroll: usize,
    pub mode: Mode,
    status: String,
    status_err: bool,
    status_at: Instant,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    nav_gen: u64,
    loading: Option<String>,
    pub size: (u16, u16),
    find: Option<FindState>,
    pub quit: bool,
    dirty: bool,
    spin: usize,
    font_shrunk: bool,
    out: Box<dyn Write>,
    pub headless: bool,
    pending_image: bool,
}

fn ctrl(k: &KeyEvent, c: char) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char(x) if x.eq_ignore_ascii_case(&c))
}

impl App {
    pub fn new(cfg: Config) -> App {
        let (tx, rx) = channel();
        let fetcher = Fetcher::new(&cfg.user_agent);
        let hist = Store::load(cfg.data_dir.join("history.tsv"));
        let marks = Store::load(cfg.data_dir.join("bookmarks.tsv"));
        let size = crossterm::terminal::size().unwrap_or((100, 30));
        App {
            cfg,
            fetcher,
            hist,
            marks,
            pages: Vec::new(),
            pos: 0,
            lay: None,
            lay_w: 0,
            focus: None,
            scroll: 0,
            mode: Mode::Browse,
            status: String::new(),
            status_err: false,
            status_at: Instant::now(),
            tx,
            rx,
            nav_gen: 0,
            loading: None,
            size,
            find: None,
            quit: false,
            dirty: true,
            spin: 0,
            font_shrunk: false,
            out: Box::new(std::io::stdout()),
            headless: false,
            pending_image: false,
        }
    }

    // ------------------------------------------------------------------
    // small helpers
    // ------------------------------------------------------------------
    pub fn say(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_err = false;
        self.status_at = Instant::now();
        self.dirty = true;
    }
    pub fn err(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_err = true;
        self.status_at = Instant::now();
        self.dirty = true;
    }
    pub fn doc(&self) -> Option<&Doc> {
        self.pages.get(self.pos).map(|p| &p.doc)
    }
    pub fn doc_mut(&mut self) -> Option<&mut Doc> {
        self.pages.get_mut(self.pos).map(|p| &mut p.doc)
    }
    pub fn layout_ref(&self) -> Option<&Layout> {
        self.lay.as_ref()
    }
    fn view_h(&self) -> usize {
        (self.size.1 as usize).saturating_sub(2).max(1)
    }
    pub fn relayout(&mut self) {
        let editing = if let Mode::Edit(w) = self.mode { Some(w) } else { None };
        if let Some(p) = self.pages.get(self.pos) {
            self.lay = Some(layout::layout(&p.doc, self.size.0 as usize, editing));
            self.lay_w = self.size.0 as usize;
        } else {
            self.lay = None;
        }
        self.clamp_scroll();
        self.dirty = true;
    }
    fn clamp_scroll(&mut self) {
        let n = self.lay.as_ref().map_or(0, |l| l.lines.len());
        let max = n.saturating_sub(self.view_h());
        if self.scroll > max {
            self.scroll = max;
        }
    }
    pub fn scroll_by(&mut self, d: isize) {
        let n = self.lay.as_ref().map_or(0, |l| l.lines.len());
        let max = n.saturating_sub(self.view_h()) as isize;
        self.scroll = (self.scroll as isize + d).clamp(0, max.max(0)) as usize;
        self.dirty = true;
    }
    pub fn scroll_to_line(&mut self, line: usize) {
        let vh = self.view_h();
        self.scroll = line.saturating_sub(vh / 3);
        self.clamp_scroll();
        self.dirty = true;
    }
    fn reveal_widget(&mut self, w: usize) {
        if let Some(Some((line, _))) = self.lay.as_ref().map(|l| l.widget_pos.get(w).cloned().flatten()) {
            let vh = self.view_h();
            if line < self.scroll || line >= self.scroll + vh {
                self.scroll = line.saturating_sub(vh / 3);
                self.clamp_scroll();
            }
        }
        self.dirty = true;
    }
    pub fn set_focus(&mut self, w: Option<usize>) {
        self.focus = w;
        if let Some(w) = w {
            self.reveal_widget(w);
        }
        self.dirty = true;
    }
    fn widget_line(&self, w: usize) -> usize {
        self.lay.as_ref().and_then(|l| l.widget_pos.get(w).cloned().flatten()).map_or(0, |p| p.0)
    }
    fn focusable(&self) -> Vec<usize> {
        let Some(l) = &self.lay else { return Vec::new() };
        let mut v: Vec<usize> = (0..l.widget_pos.len()).filter(|i| l.widget_pos[*i].is_some()).collect();
        v.sort_by_key(|i| l.widget_pos[*i]);
        v
    }

    // ------------------------------------------------------------------
    // navigation
    // ------------------------------------------------------------------
    pub fn open_input(&mut self, input: &str) {
        let target = util::resolve_input(input, &self.cfg.search);
        if target.is_empty() {
            return;
        }
        self.navigate(&target, None);
    }

    pub fn navigate(&mut self, target: &str, post: Option<Vec<(String, String)>>) {
        if let Some(doc) = pages::about(target, &self.cfg, &self.hist) {
            self.install_doc(doc);
            return;
        }
        let url = match Url::parse(target) {
            Ok(u) if matches!(u.scheme(), "http" | "https") => u,
            Ok(u) if u.scheme() == "mailto" || u.scheme() == "tel" => {
                self.say(format!("{} links open outside the terminal: {}", u.scheme(), u.path()));
                return;
            }
            Ok(u) if u.scheme() == "file" => u,
            Ok(u) => {
                self.err(format!("unsupported scheme: {}", u.scheme()));
                return;
            }
            Err(e) => {
                self.err(format!("bad URL: {e}"));
                return;
            }
        };
        if url.scheme() == "file" {
            self.open_file(&url);
            return;
        }
        self.nav_gen += 1;
        let gen = self.nav_gen;
        self.loading = Some(url.to_string());
        self.dirty = true;
        let fetcher = self.fetcher.clone();
        let tx = self.tx.clone();
        let search = self.cfg.search.clone();
        let _ = std::thread::Builder::new().stack_size(256 * 1024 * 1024).spawn(move || {
            let r = load::load(&fetcher, &url, post.as_deref(), &search);
            let _ = tx.send(Msg::Loaded { gen, result: r });
        });
    }

    fn open_file(&mut self, url: &Url) {
        let path = match url.to_file_path() {
            Ok(p) => p,
            Err(_) => {
                self.err("bad file URL");
                return;
            }
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
                    match image::load_from_memory(&bytes) {
                        Ok(img) => self.enter_image(img, url.to_string(), None),
                        Err(e) => self.err(format!("cannot decode image: {e}")),
                    }
                } else {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    let doc = if ext == "html" || ext == "htm" {
                        crate::extract::extract(url, &text)
                    } else {
                        let mut d = Doc::builder(url.clone(), path.file_name().and_then(|n| n.to_str()).unwrap_or("file"));
                        d.blocks.push(Block::Pre(text));
                        d
                    };
                    self.install_doc(doc);
                }
            }
            Err(e) => self.err(format!("{}: {e}", path.display())),
        }
    }

    fn save_position(&mut self) {
        if let Some(p) = self.pages.get_mut(self.pos) {
            p.scroll = self.scroll;
            p.focus = self.focus;
        }
    }

    pub fn install_doc(&mut self, doc: Doc) {
        self.save_position();
        if !self.pages.is_empty() {
            self.pages.truncate(self.pos + 1);
        }
        let url = doc.url.to_string();
        let title = doc.title.clone();
        let auto_edit = doc.kind == PageKind::Home || (doc.url.path() == "/" && doc.forms.iter().any(|f| f.intent == "search") && doc.blocks.len() < 8);
        let first_text = doc.widgets.iter().position(|w| w.is_text());
        let search_text = if doc.blocks.len() < 14 { doc.widgets.iter().position(|w| matches!(&w.kind, WidgetKind::Text { form: Some(f), .. } if doc.forms[*f].intent == "search")) } else { None };
        self.pages.push(Page { doc, scroll: 0, focus: None });
        if self.pages.len() > 60 {
            self.pages.remove(0);
        }
        self.pos = self.pages.len() - 1;
        {
            let t: String = title.chars().filter(|c| !c.is_control()).take(80).collect();
            let t = if t.is_empty() { "Minima".to_string() } else { format!("{t} - Minima") };
            let _ = write!(self.out, "\x1b]2;{t}\x07");
        }
        if !url.starts_with("about:") {
            self.hist.add(&url, &title);
        }
        self.loading = None;
        self.scroll = 0;
        self.focus = None;
        self.find = None;
        self.mode = Mode::Browse;
        self.relayout();
        if auto_edit {
            if let Some(w) = first_text {
                self.focus = Some(w);
                self.start_edit(w);
            }
        } else if let Some(w) = search_text {
            self.focus = Some(w);
        }
        self.dirty = true;
    }

    pub fn go_back(&mut self) {
        if self.pos > 0 {
            self.save_position();
            self.pos -= 1;
            self.restore_page();
        } else {
            self.say("no earlier page");
        }
    }

    pub fn go_forward(&mut self) {
        if self.pos + 1 < self.pages.len() {
            self.save_position();
            self.pos += 1;
            self.restore_page();
        } else {
            self.say("no later page");
        }
    }

    fn restore_page(&mut self) {
        self.mode = Mode::Browse;
        self.loading = None;
        self.nav_gen += 1; // cancel any in-flight load
        self.find = None;
        let (s, f) = {
            let p = &self.pages[self.pos];
            (p.scroll, p.focus)
        };
        self.relayout();
        self.scroll = s;
        self.focus = f;
        self.clamp_scroll();
        self.dirty = true;
    }

    pub fn reload(&mut self) {
        if let Some(d) = self.doc() {
            let u = d.url.to_string();
            if u.starts_with("about:") {
                self.navigate(&u, None);
            } else {
                // reloading replaces the current entry
                if self.pos > 0 || !self.pages.is_empty() {
                    self.pages.truncate(self.pos);
                    if self.pos > 0 {
                        self.pos -= 1;
                    }
                }
                self.navigate(&u, None);
            }
        }
    }

    // ------------------------------------------------------------------
    // forms & widgets
    // ------------------------------------------------------------------
    pub fn start_edit(&mut self, w: usize) {
        if let Some(d) = self.doc_mut() {
            if let WidgetKind::Text { value, .. } = &d.widgets[w].kind {
                d.widgets[w].cursor = value.chars().count();
            }
        }
        self.focus = Some(w);
        self.mode = Mode::Edit(w);
        self.relayout();
        self.reveal_widget(w);
    }

    pub fn activate(&mut self, w: usize) {
        let Some(d) = self.doc() else { return };
        let Some(widget) = d.widgets.get(w) else { return };
        let kind = widget.kind.clone();
        let label = widget.label.clone();
        match kind {
            WidgetKind::Link { href } => self.navigate(&href, None),
            WidgetKind::Image { src } => self.open_image(Some(w), &src),
            WidgetKind::Video { src } => self.play_video(&src, &label),
            WidgetKind::Text { .. } => self.start_edit(w),
            WidgetKind::Check { form, name, radio, .. } => {
                if let Some(d) = self.doc_mut() {
                    if radio {
                        for (i, wd) in d.widgets.iter_mut().enumerate() {
                            if let WidgetKind::Check { form: f2, name: n2, checked, radio: true, .. } = &mut wd.kind {
                                if *f2 == form && *n2 == name && i != w {
                                    *checked = false;
                                }
                            }
                        }
                    }
                    if let WidgetKind::Check { checked, .. } = &mut d.widgets[w].kind {
                        *checked = if radio { true } else { !*checked };
                    }
                }
                self.relayout();
            }
            WidgetKind::Select { options, selected, .. } => {
                let items = options.iter().map(|o| ListItem { label: o.1.clone(), detail: String::new(), key: o.0.clone() }).collect();
                self.mode = Mode::List(ListView { kind: ListKind::Options(w), title: format!("Choose: {}", label), items, sel: selected, top: 0, filter: Vec::new() });
            }
            WidgetKind::Button { form, submit, .. } => {
                if submit {
                    self.submit_form(form, Some(w));
                } else {
                    self.say("that button runs JavaScript, which Minima does not execute");
                }
            }
        }
        self.dirty = true;
    }

    pub fn submit_form(&mut self, form: Option<usize>, clicked: Option<usize>) {
        let Some(d) = self.doc() else { return };
        let Some(fid) = form else {
            self.say("this control is not inside a form");
            return;
        };
        let info = d.forms[fid].clone();
        let mut fields: Vec<(String, String)> = info.hidden.clone();
        for (i, w) in d.widgets.iter().enumerate() {
            match &w.kind {
                WidgetKind::Text { form: Some(f), name, value, .. } if *f == fid && !name.is_empty() => fields.push((name.clone(), value.clone())),
                WidgetKind::Check { form: Some(f), name, value, checked: true, .. } if *f == fid && !name.is_empty() => fields.push((name.clone(), value.clone())),
                WidgetKind::Select { form: Some(f), name, options, selected } if *f == fid && !name.is_empty() => {
                    if let Some(o) = options.get(*selected) {
                        fields.push((name.clone(), o.0.clone()));
                    }
                }
                WidgetKind::Button { form: Some(f), name, value, .. } if *f == fid && !name.is_empty() && Some(i) == clicked => fields.push((name.clone(), value.clone())),
                _ => {}
            }
        }
        let base = match Url::parse(&info.action) {
            Ok(u) => u,
            Err(_) => {
                self.err("bad form action");
                return;
            }
        };
        match info.method {
            Method::Get => {
                let u = fetch::query_pairs_url(&base, &fields);
                self.navigate(u.as_str(), None);
            }
            Method::Post => self.navigate(base.as_str(), Some(fields)),
        }
    }

    pub fn submit_focused_or_first_form(&mut self) {
        let Some(d) = self.doc() else { return };
        let form = self
            .focus
            .and_then(|w| match &d.widgets[w].kind {
                WidgetKind::Text { form, .. } | WidgetKind::Check { form, .. } | WidgetKind::Select { form, .. } | WidgetKind::Button { form, .. } => *form,
                _ => None,
            })
            .or_else(|| if d.forms.is_empty() { None } else { Some(0) });
        self.submit_form(form, None);
    }

    // ------------------------------------------------------------------
    // images / video / downloads
    // ------------------------------------------------------------------
    fn image_widgets(&self) -> Vec<usize> {
        let Some(d) = self.doc() else { return Vec::new() };
        let mut v: Vec<usize> = d.widgets.iter().enumerate().filter(|(_, w)| matches!(w.kind, WidgetKind::Image { .. })).map(|(i, _)| i).collect();
        if let Some(l) = &self.lay {
            v.sort_by_key(|i| l.widget_pos[*i]);
        }
        v
    }

    pub fn open_image(&mut self, from: Option<usize>, src: &str) {
        let url = match Url::parse(src) {
            Ok(u) => u,
            Err(_) => {
                self.err("bad image URL");
                return;
            }
        };
        self.nav_gen += 1;
        let gen = self.nav_gen;
        self.pending_image = true;
        self.say(format!("loading image {}…", truncate_width(src, 60)));
        let fetcher = self.fetcher.clone();
        let tx = self.tx.clone();
        let s = src.to_string();
        let _ = std::thread::Builder::new().stack_size(64 * 1024 * 1024).spawn(move || {
            let result = if url.scheme() == "file" {
                url.to_file_path().map_err(|_| "bad file path".to_string()).and_then(|p| image::open(p).map_err(|e| e.to_string()))
            } else {
                fetcher.get(&url).and_then(|r| {
                    if r.body.is_empty() {
                        return Err(format!("HTTP {} (empty)", r.status));
                    }
                    image::load_from_memory(&r.body).map_err(|e| format!("cannot decode image ({}): {e}", r.content_type))
                })
            };
            let _ = tx.send(Msg::Image { gen, result, from_widget: from, src: s });
        });
    }

    fn enter_image(&mut self, img: DynamicImage, src: String, from: Option<usize>) {
        let siblings = self.image_widgets();
        let idx = from.and_then(|w| siblings.iter().position(|x| *x == w)).unwrap_or(0);
        let mut v = ImageView { img, src, mode: self.cfg.image_mode, color: self.cfg.color, lines: Vec::new(), cells_w: 0, zoom: 1.0, siblings, idx };
        if self.cfg.font_shrink && !self.font_shrunk {
            let _ = self.out.write_all(b"\x1b]50;#-2\x07");
            self.font_shrunk = true;
        }
        self.size = self.term_size();
        Self::render_image_view(&mut v, self.size);
        self.mode = Mode::Image(v);
        let _ = self.out.write_all(b"\x1b[2J");
        self.dirty = true;
    }

    fn render_image_view(v: &mut ImageView, size: (u16, u16)) {
        let max_cols = ((size.0 as f32) * v.zoom).max(8.0) as usize;
        let max_rows = (((size.1 as usize).saturating_sub(2)) as f32 * v.zoom).max(4.0) as usize;
        v.cells_w = ascii::fit_cells(v.mode, v.img.width(), v.img.height(), max_cols, max_rows).0;
        v.lines = ascii::render_image(&v.img, v.mode, v.color, max_cols, max_rows);
    }

    fn leave_image(&mut self) {
        if self.font_shrunk {
            let _ = self.out.write_all(b"\x1b]50;#+2\x07");
            self.font_shrunk = false;
        }
        self.mode = Mode::Browse;
        self.size = self.term_size();
        let _ = self.out.write_all(b"\x1b[2J");
        self.relayout();
    }

    pub fn play_video(&mut self, src: &str, title: &str) {
        self.say(format!("starting video: {}", truncate_width(title, 50)));
        self.draw();
        let _ = self.out.write_all(b"\x1b[2J\x1b[?25l");
        if self.cfg.font_shrink && !self.font_shrunk {
            let _ = self.out.write_all(b"\x1b]50;#-2\x07");
            self.font_shrunk = true;
        }
        let mut out = std::io::stdout();
        let r = media::play(src, title, &self.cfg, &self.fetcher, &mut out);
        if self.font_shrunk {
            let _ = out.write_all(b"\x1b]50;#+2\x07");
            self.font_shrunk = false;
        }
        let _ = out.write_all(b"\x1b[0m\x1b[2J");
        self.size = self.term_size();
        match r {
            Ok(_) => self.say("video closed"),
            Err(e) => self.err(e),
        }
        self.relayout();
    }

    pub fn download_target(&mut self, w: Option<usize>) {
        let (src, is_video) = match w.and_then(|w| self.doc().map(|d| d.widgets[w].kind.clone())) {
            Some(WidgetKind::Link { href }) => (href, false),
            Some(WidgetKind::Image { src }) => (src, false),
            Some(WidgetKind::Video { src }) => (src, true),
            _ => match self.doc() {
                Some(d) => (d.url.to_string(), media::youtube_id(&d.url).is_some()),
                None => return,
            },
        };
        self.download(&src, is_video);
    }

    pub fn download(&mut self, src: &str, is_video: bool) {
        let tools = media::tools();
        let tx = self.tx.clone();
        let dir = self.cfg.download_dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        let video_site = Url::parse(src).ok().and_then(|u| u.host_str().map(|h| media::is_youtube_host(h) || h.contains("vimeo.com"))).unwrap_or(false);
        if is_video || video_site {
            match media::spawn_ytdlp_download(src, &dir, &tools) {
                Ok(mut child) => {
                    self.say("downloading video with yt-dlp…");
                    let stdout = child.stdout.take();
                    let dir2 = dir.clone();
                    std::thread::spawn(move || {
                        use std::io::BufRead;
                        if let Some(o) = stdout {
                            for line in std::io::BufReader::new(o).lines().map_while(Result::ok) {
                                if line.contains("[download]") && line.contains('%') {
                                    let _ = tx.send(Msg::Status(format!("downloading: {}", line.replace("[download]", "").trim()), false));
                                }
                            }
                        }
                        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
                        let _ = tx.send(if ok { Msg::Status(format!("video saved to {}", dir2.display()), false) } else { Msg::Status("video download failed".into(), true) });
                    });
                }
                Err(e) => self.err(e),
            }
            return;
        }
        let url = match Url::parse(src) {
            Ok(u) if matches!(u.scheme(), "http" | "https") => u,
            _ => {
                self.err("nothing downloadable here");
                return;
            }
        };
        let fetcher = self.fetcher.clone();
        let name = util::filename_from_url(&url);
        let path = unique_path(&dir, &name);
        self.say(format!("downloading {}…", name));
        std::thread::spawn(move || {
            let txp = tx.clone();
            let r = fetcher.download(&url, &path, |done, total| {
                let msg = match total {
                    Some(t) if t > 0 => format!("downloading {} — {} / {} ({}%)", name_of(&path), util::human_bytes(done), util::human_bytes(t), done * 100 / t),
                    _ => format!("downloading {} — {}", name_of(&path), util::human_bytes(done)),
                };
                let _ = txp.send(Msg::Status(msg, false));
            });
            let _ = tx.send(match r {
                Ok(n) => Msg::Status(format!("saved {} ({}) → {}", name_of(&path), util::human_bytes(n), path.display()), false),
                Err(e) => Msg::Status(format!("download failed: {e}"), true),
            });
        });
    }

    // ------------------------------------------------------------------
    // overlays
    // ------------------------------------------------------------------
    pub fn show_history(&mut self) {
        let items = self.hist.recent().into_iter().take(2000).map(|e| ListItem { label: if e.title.is_empty() { e.url.clone() } else { e.title.clone() }, detail: e.url.clone(), key: e.url }).collect();
        self.mode = Mode::List(ListView { kind: ListKind::History, title: "History  (type to filter · Enter open · Del remove · Esc close)".into(), items, sel: 0, top: 0, filter: Vec::new() });
        self.dirty = true;
    }

    pub fn show_bookmarks(&mut self) {
        let items = self.marks.recent().into_iter().map(|e| ListItem { label: if e.title.is_empty() { e.url.clone() } else { e.title.clone() }, detail: e.url.clone(), key: e.url }).collect();
        self.mode = Mode::List(ListView { kind: ListKind::Bookmarks, title: "Bookmarks  (type to filter · Enter open · Del remove · Esc close)".into(), items, sel: 0, top: 0, filter: Vec::new() });
        self.dirty = true;
    }

    pub fn show_links(&mut self) {
        let Some(d) = self.doc() else { return };
        let items: Vec<ListItem> = d
            .widgets
            .iter()
            .enumerate()
            .filter_map(|(i, w)| match &w.kind {
                WidgetKind::Link { href } => Some(ListItem { label: w.label.clone(), detail: href.clone(), key: i.to_string() }),
                WidgetKind::Button { submit: true, .. } => Some(ListItem { label: format!("[ {} ]", w.label), detail: "button".into(), key: i.to_string() }),
                WidgetKind::Text { .. } => Some(ListItem { label: format!("field: {}", w.label), detail: "input".into(), key: i.to_string() }),
                _ => None,
            })
            .collect();
        self.mode = Mode::List(ListView { kind: ListKind::Links, title: "Links & controls  (type to filter · Enter go · Esc close)".into(), items, sel: 0, top: 0, filter: Vec::new() });
        self.dirty = true;
    }

    pub fn show_images(&mut self) {
        let ids = self.image_widgets();
        let Some(d) = self.doc() else { return };
        let items: Vec<ListItem> = ids
            .into_iter()
            .map(|i| {
                let src = if let WidgetKind::Image { src } = &d.widgets[i].kind { src.clone() } else { String::new() };
                ListItem { label: d.widgets[i].label.clone(), detail: src, key: i.to_string() }
            })
            .collect();
        if items.is_empty() {
            self.say("no images on this page");
            return;
        }
        self.mode = Mode::List(ListView { kind: ListKind::Images, title: "Images  (Enter view as ASCII art · Esc close)".into(), items, sel: 0, top: 0, filter: Vec::new() });
        self.dirty = true;
    }

    fn list_choose(&mut self) {
        let Mode::List(lv) = &self.mode else { return };
        let vis = lv.visible();
        let Some(&idx) = vis.get(lv.sel) else { return };
        let kind = lv.kind;
        let key = lv.items[idx].key.clone();
        self.mode = Mode::Browse;
        match kind {
            ListKind::History | ListKind::Bookmarks => self.navigate(&key, None),
            ListKind::Links => {
                if let Ok(w) = key.parse::<usize>() {
                    self.set_focus(Some(w));
                    self.activate(w);
                }
            }
            ListKind::Images => {
                if let Ok(w) = key.parse::<usize>() {
                    self.set_focus(Some(w));
                    self.activate(w);
                }
            }
            ListKind::Options(w) => {
                let opt_idx = idx;
                if let Some(d) = self.doc_mut() {
                    if let WidgetKind::Select { selected, .. } = &mut d.widgets[w].kind {
                        *selected = opt_idx;
                    }
                }
                self.relayout();
            }
        }
        self.dirty = true;
    }

    pub fn toggle_bookmark(&mut self) {
        let Some(d) = self.doc() else { return };
        let (url, title) = (d.url.to_string(), d.title.clone());
        if url.starts_with("about:") {
            return;
        }
        if self.marks.contains(&url) {
            self.marks.remove(&url);
            self.say("bookmark removed");
        } else {
            self.marks.add(&url, &title);
            self.say(format!("bookmarked: {}", truncate_width(&title, 50)));
        }
    }

    // ------------------------------------------------------------------
    // find / prompts
    // ------------------------------------------------------------------
    pub fn open_prompt(&mut self, kind: PromptKind, prefill: &str) {
        let buf: Vec<char> = prefill.chars().collect();
        let cur = buf.len();
        self.mode = Mode::Prompt(Prompt { kind, buf, cur });
        self.dirty = true;
    }

    pub fn do_find(&mut self, q: &str) {
        let ql = q.to_lowercase();
        let Some(l) = &self.lay else { return };
        let lines: Vec<usize> = l.lines.iter().enumerate().filter(|(_, ln)| ln.text().to_lowercase().contains(&ql)).map(|(i, _)| i).collect();
        if lines.is_empty() {
            self.err(format!("not found: {q}"));
            self.find = None;
            return;
        }
        let start = lines.iter().position(|&i| i >= self.scroll + 1).unwrap_or(0);
        let target = lines[start];
        self.say(format!("{} matches for “{}” — n / N for next / previous", lines.len(), q));
        self.find = Some(FindState { query: q.to_string(), lines, idx: start });
        self.scroll_to_line(target);
    }

    fn find_step(&mut self, dir: isize) {
        let Some(f) = &mut self.find else {
            self.say("nothing to find (press /)");
            return;
        };
        let n = f.lines.len() as isize;
        f.idx = ((f.idx as isize + dir).rem_euclid(n)) as usize;
        let line = f.lines[f.idx];
        let msg = format!("match {}/{} for “{}”", f.idx + 1, n, f.query);
        self.say(msg);
        self.scroll_to_line(line);
    }

    // ------------------------------------------------------------------
    // key handling
    // ------------------------------------------------------------------
    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.handle_key(k),
            Event::Mouse(m) => self.handle_mouse(m),
            Event::Resize(w, h) => {
                self.size = (w, h);
                if let Mode::Image(v) = &mut self.mode {
                    Self::render_image_view(v, (w, h));
                    let _ = self.out.write_all(b"\x1b[2J");
                }
                self.relayout();
                let _ = self.out.write_all(b"\x1b[2J");
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn handle_mouse(&mut self, m: MouseEvent) {
        if !matches!(self.mode, Mode::Browse | Mode::Edit(_)) {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollDown => self.scroll_by(3),
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::Down(MouseButton::Left) => {
                if m.row == 0 {
                    // top bar: click the arrows
                    if m.column <= 2 {
                        self.go_back();
                    } else if m.column <= 5 {
                        self.go_forward();
                    } else {
                        self.open_prompt(PromptKind::Url, "");
                    }
                    return;
                }
                let line = self.scroll + (m.row as usize).saturating_sub(1);
                let hit = self.lay.as_ref().and_then(|l| {
                    let ln = l.lines.get(line)?;
                    let mut col = 0usize;
                    for s in &ln.segs {
                        let w = width(&s.text);
                        if (m.column as usize) >= col && (m.column as usize) < col + w {
                            return s.widget;
                        }
                        col += w;
                    }
                    None
                });
                if let Some(w) = hit {
                    if let Mode::Edit(_) = self.mode {
                        self.mode = Mode::Browse;
                    }
                    self.focus = Some(w);
                    self.activate(w);
                }
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, k: KeyEvent) {
        if ctrl(&k, 'c') {
            self.quit = true;
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Browse);
        match mode {
            Mode::Browse => self.key_browse(k),
            Mode::Edit(w) => {
                self.mode = Mode::Edit(w);
                self.key_edit(w, k)
            }
            Mode::Prompt(p) => self.key_prompt(p, k),
            Mode::List(l) => self.key_list(l, k),
            Mode::Image(v) => self.key_image(v, k),
        }
    }

    fn key_browse(&mut self, k: KeyEvent) {
        let vh = self.view_h() as isize;
        // A focused text field takes typing directly: no need to press Enter first.
        if let (Some(w), KeyCode::Char(_)) = (self.focus, k.code) {
            let is_text = matches!(self.doc().and_then(|d| d.widgets.get(w)).map(|x| &x.kind), Some(WidgetKind::Text { .. }));
            if is_text && !k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT) {
                self.start_edit(w);
                self.key_edit(w, k);
                return;
            }
        }
        if self.loading.is_some() && k.code == KeyCode::Esc {
            self.loading = None;
            self.nav_gen += 1;
            self.say("cancelled");
            return;
        }
        match k.code {
            KeyCode::Char('q') if k.modifiers.is_empty() => self.quit = true,
            KeyCode::Char('g') if k.modifiers.is_empty() => self.open_prompt(PromptKind::Url, ""),
            KeyCode::Char('G') | KeyCode::F(6) => {
                let u = self.doc().map(|d| d.url.to_string()).unwrap_or_default();
                self.open_prompt(PromptKind::Url, &u);
            }
            KeyCode::Char('l') if ctrl(&k, 'l') => {
                let u = self.doc().map(|d| d.url.to_string()).unwrap_or_default();
                self.open_prompt(PromptKind::Url, &u);
            }
            KeyCode::Char(':') | KeyCode::Char(';') => self.open_prompt(PromptKind::Command, ""),
            KeyCode::Char('/') => self.open_prompt(PromptKind::Find, ""),
            KeyCode::Char('n') if k.modifiers.is_empty() => self.find_step(1),
            KeyCode::Char('N') => self.find_step(-1),
            KeyCode::Char('?') | KeyCode::F(1) => self.navigate("about:help", None),
            KeyCode::Char('r') | KeyCode::F(5) if k.modifiers.is_empty() || matches!(k.code, KeyCode::F(5)) => self.reload(),
            KeyCode::Char('h') if k.modifiers.is_empty() => self.go_back(),
            KeyCode::Char('H') => self.show_history(),
            KeyCode::Char('h') if ctrl(&k, 'h') => self.show_history(),
            KeyCode::Char('b') if k.modifiers.is_empty() => self.toggle_bookmark(),
            KeyCode::Char('B') => self.show_bookmarks(),
            KeyCode::Char('L') => self.show_links(),
            KeyCode::Char('I') => self.show_images(),
            KeyCode::Char('~') => self.navigate("about:home", None),
            KeyCode::Char('i') => {
                let ids = self.image_widgets();
                let pick = self.focus.filter(|f| ids.contains(f)).or_else(|| {
                    // first image at/after the top of the view
                    ids.iter().cloned().find(|i| self.widget_line(*i) >= self.scroll).or(ids.first().cloned())
                });
                match pick {
                    Some(w) => {
                        self.set_focus(Some(w));
                        self.activate(w);
                    }
                    None => self.say("no images on this page"),
                }
            }
            KeyCode::Char('p') => {
                let pick = self.focus.filter(|f| matches!(self.doc().map(|d| &d.widgets[*f].kind), Some(WidgetKind::Video { .. }))).or_else(|| self.doc().and_then(|d| d.widgets.iter().position(|w| matches!(w.kind, WidgetKind::Video { .. }))));
                match pick {
                    Some(w) => self.activate(w),
                    None => self.say("no video on this page"),
                }
            }
            KeyCode::Char('d') => {
                let f = self.focus;
                self.download_target(f);
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let back = k.code == KeyCode::BackTab || k.modifiers.contains(KeyModifiers::SHIFT);
                self.focus_step(if back { -1 } else { 1 });
            }
            KeyCode::Down => self.move_vertical(1),
            KeyCode::Up => self.move_vertical(-1),
            KeyCode::Left => {
                if k.modifiers.contains(KeyModifiers::ALT) || !self.move_horizontal(-1) {
                    self.go_back();
                }
            }
            KeyCode::Right => {
                if k.modifiers.contains(KeyModifiers::ALT) {
                    self.go_forward();
                } else if !self.move_horizontal(1) {
                    self.go_forward();
                }
            }
            KeyCode::Backspace => self.go_back(),
            KeyCode::Enter => match self.focus {
                Some(w) => self.activate(w),
                None => self.focus_step(1),
            },
            KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(vh - 1),
            KeyCode::PageUp => self.scroll_by(-(vh - 1)),
            KeyCode::Home => {
                self.scroll = 0;
                self.dirty = true;
            }
            KeyCode::End => {
                self.scroll = usize::MAX / 2;
                self.clamp_scroll();
                self.dirty = true;
            }
            KeyCode::Esc => {
                self.focus = None;
                self.find = None;
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn focus_step(&mut self, dir: isize) {
        let f = self.focusable();
        if f.is_empty() {
            self.say("nothing to select on this page");
            return;
        }
        let cur = self.focus.and_then(|w| f.iter().position(|x| *x == w));
        let next = match cur {
            Some(i) => (i as isize + dir).rem_euclid(f.len() as isize) as usize,
            None => {
                // start from whatever is visible
                let vis = f.iter().position(|w| self.widget_line(*w) >= self.scroll).unwrap_or(0);
                if dir > 0 {
                    vis
                } else {
                    vis.saturating_sub(1)
                }
            }
        };
        self.set_focus(Some(f[next]));
    }

    fn move_vertical(&mut self, dir: isize) {
        let f = self.focusable();
        let cur_line = self.focus.map(|w| self.widget_line(w));
        let target = match cur_line {
            Some(cl) => {
                if dir > 0 {
                    f.iter().cloned().find(|w| self.widget_line(*w) > cl)
                } else {
                    f.iter().cloned().rev().find(|w| self.widget_line(*w) < cl)
                }
            }
            None => {
                let lo = self.scroll;
                let hi = self.scroll + self.view_h();
                if dir > 0 {
                    f.iter().cloned().find(|w| (lo..hi).contains(&self.widget_line(*w)))
                } else {
                    f.iter().cloned().rev().find(|w| (lo..hi).contains(&self.widget_line(*w)))
                }
            }
        };
        match target {
            Some(w) => self.set_focus(Some(w)),
            None => self.scroll_by(dir),
        }
    }

    /// Move to the neighbouring widget on the same line. Returns false if there is none.
    fn move_horizontal(&mut self, dir: isize) -> bool {
        let Some(cur) = self.focus else { return false };
        let Some(l) = &self.lay else { return false };
        let Some((line, col)) = l.widget_pos[cur] else { return false };
        let mut row: Vec<(usize, usize)> = (0..l.widget_pos.len()).filter_map(|w| l.widget_pos[w].filter(|p| p.0 == line).map(|p| (p.1, w))).collect();
        row.sort();
        let cand = if dir > 0 { row.into_iter().find(|(c, w)| *c > col && *w != cur) } else { row.into_iter().rev().find(|(c, w)| *c < col && *w != cur) };
        match cand {
            Some((_, w)) => {
                self.set_focus(Some(w));
                true
            }
            None => false,
        }
    }

    fn key_edit(&mut self, w: usize, k: KeyEvent) {
        let multiline = matches!(self.doc().map(|d| &d.widgets[w].kind), Some(WidgetKind::Text { multiline: true, .. }));
        let form = match self.doc().map(|d| &d.widgets[w].kind) {
            Some(WidgetKind::Text { form, .. }) => *form,
            _ => None,
        };
        let mut leave = false;
        let mut submit = false;
        let mut step: Option<isize> = None;
        {
            let Some(d) = self.doc_mut() else { return };
            let wref = &mut d.widgets[w];
            let cursor = &mut wref.cursor;
            if let WidgetKind::Text { value, .. } = &mut wref.kind {
                let mut chars: Vec<char> = value.chars().collect();
                *cursor = (*cursor).min(chars.len());
                match k.code {
                    KeyCode::Esc => leave = true,
                    KeyCode::Enter => {
                        if multiline && !k.modifiers.contains(KeyModifiers::CONTROL) {
                            chars.insert(*cursor, '\n');
                            *cursor += 1;
                        } else {
                            submit = true;
                        }
                    }
                    KeyCode::Tab => step = Some(1),
                    KeyCode::BackTab => step = Some(-1),
                    KeyCode::Down => step = Some(1),
                    KeyCode::Up => step = Some(-1),
                    KeyCode::Left => *cursor = cursor.saturating_sub(1),
                    KeyCode::Right => *cursor = (*cursor + 1).min(chars.len()),
                    KeyCode::Home => *cursor = 0,
                    KeyCode::End => *cursor = chars.len(),
                    KeyCode::Backspace => {
                        if *cursor > 0 {
                            chars.remove(*cursor - 1);
                            *cursor -= 1;
                        }
                    }
                    KeyCode::Delete => {
                        if *cursor < chars.len() {
                            chars.remove(*cursor);
                        }
                    }
                    KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        chars.drain(..*cursor);
                        *cursor = 0;
                    }
                    KeyCode::Char('k') if k.modifiers.contains(KeyModifiers::CONTROL) => chars.truncate(*cursor),
                    KeyCode::Char('a') if k.modifiers.contains(KeyModifiers::CONTROL) => *cursor = 0,
                    KeyCode::Char('e') if k.modifiers.contains(KeyModifiers::CONTROL) => *cursor = chars.len(),
                    KeyCode::Char('w') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        let mut i = *cursor;
                        while i > 0 && chars[i - 1] == ' ' {
                            i -= 1;
                        }
                        while i > 0 && chars[i - 1] != ' ' {
                            i -= 1;
                        }
                        chars.drain(i..*cursor);
                        *cursor = i;
                    }
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT) => {
                        chars.insert(*cursor, c);
                        *cursor += 1;
                    }
                    _ => {}
                }
                *value = chars.into_iter().collect();
            }
        }
        if leave {
            self.mode = Mode::Browse;
        } else if let Some(d) = step {
            self.mode = Mode::Browse;
            self.focus_step(d);
        } else if submit {
            self.mode = Mode::Browse;
            self.submit_form(form, None);
            return;
        }
        self.relayout();
    }

    fn key_prompt(&mut self, mut p: Prompt, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.dirty = true;
                return;
            }
            KeyCode::Enter => {
                let text: String = p.buf.iter().collect();
                match p.kind {
                    PromptKind::Url => self.open_input(&text),
                    PromptKind::Find => self.do_find(&text),
                    PromptKind::Command => command::run(self, &text),
                }
                return;
            }
            KeyCode::Left => p.cur = p.cur.saturating_sub(1),
            KeyCode::Right => p.cur = (p.cur + 1).min(p.buf.len()),
            KeyCode::Home => p.cur = 0,
            KeyCode::End => p.cur = p.buf.len(),
            KeyCode::Backspace => {
                if p.cur > 0 {
                    p.buf.remove(p.cur - 1);
                    p.cur -= 1;
                } else if p.buf.is_empty() {
                    return;
                }
            }
            KeyCode::Delete => {
                if p.cur < p.buf.len() {
                    p.buf.remove(p.cur);
                }
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                p.buf.drain(..p.cur);
                p.cur = 0;
            }
            KeyCode::Char('w') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                let mut i = p.cur;
                while i > 0 && p.buf[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && p.buf[i - 1] != ' ' {
                    i -= 1;
                }
                p.buf.drain(i..p.cur);
                p.cur = i;
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                p.buf.insert(p.cur, c);
                p.cur += 1;
            }
            _ => {}
        }
        self.mode = Mode::Prompt(p);
    }

    fn key_list(&mut self, mut l: ListView, k: KeyEvent) {
        let vis = l.visible();
        let page = (self.view_h().saturating_sub(3)).max(3);
        match k.code {
            KeyCode::Esc => {
                self.dirty = true;
                return;
            }
            KeyCode::Enter => {
                self.mode = Mode::List(l);
                self.list_choose();
                return;
            }
            KeyCode::Up => l.sel = l.sel.saturating_sub(1),
            KeyCode::Down => l.sel = (l.sel + 1).min(vis.len().saturating_sub(1)),
            KeyCode::PageUp => l.sel = l.sel.saturating_sub(page),
            KeyCode::PageDown => l.sel = (l.sel + page).min(vis.len().saturating_sub(1)),
            KeyCode::Home => l.sel = 0,
            KeyCode::End => l.sel = vis.len().saturating_sub(1),
            KeyCode::Delete => {
                if let Some(&idx) = vis.get(l.sel) {
                    let key = l.items[idx].key.clone();
                    match l.kind {
                        ListKind::History => self.hist.remove(&key),
                        ListKind::Bookmarks => self.marks.remove(&key),
                        _ => {}
                    }
                    if matches!(l.kind, ListKind::History | ListKind::Bookmarks) {
                        l.items.remove(idx);
                    }
                }
            }
            KeyCode::Backspace => {
                l.filter.pop();
                l.sel = 0;
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                l.filter.push(c);
                l.sel = 0;
            }
            _ => {}
        }
        self.mode = Mode::List(l);
    }

    fn key_image(&mut self, mut v: ImageView, k: KeyEvent) {
        let mut rerender = false;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter | KeyCode::Backspace => {
                self.mode = Mode::Image(v);
                self.leave_image();
                return;
            }
            KeyCode::Char('m') => {
                v.mode = v.mode.next();
                rerender = true;
            }
            KeyCode::Char('c') => {
                v.color = !v.color;
                rerender = true;
            }
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Up => {
                v.zoom = (v.zoom * 1.0).min(1.0);
                v.zoom = (v.zoom + 0.0).min(1.0);
            }
            KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Down => {
                v.zoom = (v.zoom * 0.8).max(0.2);
                rerender = true;
            }
            KeyCode::Char('0') => {
                v.zoom = 1.0;
                rerender = true;
            }
            KeyCode::Char('f') => {
                if self.font_shrunk {
                    let _ = self.out.write_all(b"\x1b]50;#+2\x07");
                } else {
                    let _ = self.out.write_all(b"\x1b]50;#-2\x07");
                }
                self.font_shrunk = !self.font_shrunk;
                self.say("font size request sent (works in xterm-compatible terminals; elsewhere use Ctrl+- to shrink the font)");
            }
            KeyCode::Char('d') | KeyCode::Char('s') => {
                let src = v.src.clone();
                self.download(&src, false);
            }
            KeyCode::Left | KeyCode::Right => {
                if v.siblings.len() > 1 {
                    let n = v.siblings.len() as isize;
                    let next = (v.idx as isize + if k.code == KeyCode::Right { 1 } else { -1 }).rem_euclid(n) as usize;
                    let w = v.siblings[next];
                    let src = match self.doc().map(|d| d.widgets[w].kind.clone()) {
                        Some(WidgetKind::Image { src }) => src,
                        _ => String::new(),
                    };
                    if !src.is_empty() {
                        self.mode = Mode::Image(v);
                        self.open_image(Some(w), &src);
                        return;
                    }
                }
            }
            _ => {}
        }
        if rerender {
            Self::render_image_view(&mut v, self.size);
            let _ = self.out.write_all(b"\x1b[2J");
        }
        self.mode = Mode::Image(v);
    }

    // ------------------------------------------------------------------
    // messages from worker threads
    // ------------------------------------------------------------------
    fn handle_msg(&mut self, m: Msg) {
        match m {
            Msg::Status(s, e) => {
                if e {
                    self.err(s)
                } else {
                    self.say(s)
                }
            }
            Msg::Loaded { gen, result } => {
                if gen != self.nav_gen {
                    return;
                }
                self.loading = None;
                match result {
                    Ok(Loaded::Doc(d)) => self.install_doc(d),
                    Ok(Loaded::Image(img, url)) => self.enter_image(img, url.to_string(), None),
                    Ok(Loaded::Video(u)) => {
                        let s = u.to_string();
                        self.play_video(&s, &s);
                    }
                    Ok(Loaded::Binary(u, ct)) => {
                        let s = u.to_string();
                        self.say(format!("{} ({}) is a file — downloading", util::filename_from_url(&u), if ct.is_empty() { "binary" } else { &ct }));
                        self.download(&s, false);
                    }
                    Err(e) => {
                        self.err(format!("failed to load: {}", fetch::friendly_error(&e)));
                    }
                }
                self.dirty = true;
            }
            Msg::Image { gen, result, from_widget, src } => {
                self.pending_image = false;
                if gen != self.nav_gen {
                    return;
                }
                match result {
                    Ok(img) => {
                        let prev_idx_font = self.font_shrunk;
                        let _ = prev_idx_font;
                        self.enter_image(img, src, from_widget);
                    }
                    Err(e) => self.err(format!("image: {e}")),
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // drawing
    // ------------------------------------------------------------------
    pub fn draw(&mut self) {
        let s = self.render();
        let _ = self.out.write_all(s.as_bytes());
        let _ = self.out.flush();
    }

    pub fn term_size(&self) -> (u16, u16) {
        if self.headless {
            self.size
        } else {
            crossterm::terminal::size().unwrap_or(self.size)
        }
    }

    pub fn render(&mut self) -> String {
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        if w < 10 || h < 4 {
            return String::new();
        }
        if self.lay.is_none() || self.lay_w != w {
            self.relayout();
        }
        let mut s = String::with_capacity(w * h * 4);
        s.push_str("\x1b[?2026h\x1b[?25l");
        let mut cursor_at: Option<(usize, usize)> = None;

        if let Mode::Image(v) = &self.mode {
            let x0 = w.saturating_sub(v.cells_w) / 2;
            let y0 = (h.saturating_sub(1)).saturating_sub(v.lines.len()) / 2;
            for r in 0..h - 1 {
                use std::fmt::Write as _;
                let _ = write!(s, "\x1b[{};1H\x1b[2K", r + 1);
                if r >= y0 && r - y0 < v.lines.len() {
                    let _ = write!(s, "\x1b[{};{}H{}\x1b[0m", r + 1, x0 + 1, v.lines[r - y0]);
                }
            }
            let status = format!(
                " {}×{}  [{}{}] zoom {:.0}%  │ m mode  c color  -/0 zoom  ←/→ prev/next image  d save  f font  Esc back",
                v.img.width(),
                v.img.height(),
                v.mode.name(),
                if v.color { "+color" } else { "" },
                v.zoom * 100.0
            );
            use std::fmt::Write as _;
            let _ = write!(s, "\x1b[{};1H\x1b[0m\x1b[7m{:<w$}\x1b[0m", h, truncate_width(&status, w), w = w);
            s.push_str("\x1b[?2026l");
            self.dirty = false;
            return s;
        }

        // ---- top bar
        {
            use std::fmt::Write as _;
            let (url, kind) = match self.doc() {
                Some(d) => (d.url.to_string(), d.kind.name()),
                None => (String::new(), ""),
            };
            let back = if self.pos > 0 { "◀" } else { "·" };
            let fwd = if self.pos + 1 < self.pages.len() { "▶" } else { "·" };
            let book = if self.marks.contains(&url) { "★" } else { " " };
            let url_disp = truncate_width(&url, w.saturating_sub(18));
            let bar = format!(" {back} {fwd}  {url_disp}");
            let right = format!("{kind} {book} ");
            let pad = w.saturating_sub(width(&bar) + width(&right));
            let top = if self.cfg.color { "\x1b[1;97;44m" } else { "\x1b[7m" };
            let _ = write!(s, "\x1b[1;1H\x1b[0m{}{}{}{}\x1b[0m", top, bar, " ".repeat(pad), right);
        }

        // ---- content or list
        let vh = h - 2;
        match &self.mode {
            Mode::List(l) => self.draw_list(&mut s, l, w, vh),
            _ => {
                if let Some(lay) = &self.lay {
                    let editing = if let Mode::Edit(w) = &self.mode { Some(*w) } else { None };
                    let find_lines: &[usize] = self.find.as_ref().map_or(&[], |f| &f.lines);
                    for r in 0..vh {
                        use std::fmt::Write as _;
                        let li = self.scroll + r;
                        let _ = write!(s, "\x1b[{};1H\x1b[0m", r + 2);
                        if let Some(line) = lay.lines.get(li) {
                            if find_lines.binary_search(&li).is_ok() {
                                s.push_str("\x1b[93m▌\x1b[0m\x1b[1D");
                            }
                            let mut col = 0usize;
                            for seg in &line.segs {
                                let focused = seg.widget.is_some() && seg.widget == self.focus;
                                let code = self.sgr(seg.style, focused, editing.is_some() && editing == seg.widget);
                                let tw = width(&seg.text);
                                if col + tw > w {
                                    let t = truncate_width(&seg.text, w.saturating_sub(col));
                                    let _ = write!(s, "\x1b[0m{}{}", code, t);
                                    col = w;
                                    break;
                                }
                                let _ = write!(s, "\x1b[0m{}{}", code, seg.text);
                                col += tw;
                            }
                            s.push_str("\x1b[0m\x1b[K");
                        } else {
                            s.push_str("\x1b[K");
                        }
                    }
                    if let Some(ed) = editing {
                        if let Some(Some((line, col))) = Some(lay.widget_pos.get(ed).cloned().flatten()) {
                            if line >= self.scroll && line < self.scroll + vh {
                                let cur = self.doc().map(|d| d.widgets[ed].cursor).unwrap_or(0);
                                let (value, password) = match self.doc().map(|d| &d.widgets[ed].kind) {
                                    Some(WidgetKind::Text { value, password, .. }) => (value.clone(), *password),
                                    _ => (String::new(), false),
                                };
                                let shown: String = if password { "•".repeat(value.chars().count()) } else { value.replace('\n', "⏎") };
                                let fw = layout_field_inner_width(lay, line, col);
                                let before: String = shown.chars().take(cur).collect();
                                let bw = width(&before);
                                let off = if bw >= fw { fw.saturating_sub(1) } else { bw };
                                cursor_at = Some((line - self.scroll + 2, col + 2 + off));
                            }
                        }
                    }
                } else {
                    for r in 0..vh {
                        use std::fmt::Write as _;
                        let _ = write!(s, "\x1b[{};1H\x1b[K", r + 2);
                    }
                    if self.loading.is_some() {
                        s.push_str("\x1b[3;3H\x1b[2mloading…\x1b[0m");
                    }
                }
            }
        }

        // ---- bottom bar
        {
            use std::fmt::Write as _;
            let _ = write!(s, "\x1b[{};1H\x1b[0m\x1b[2K", h);
            match &self.mode {
                Mode::Prompt(p) => {
                    let label = match p.kind {
                        PromptKind::Url => "Go to (URL or search)",
                        PromptKind::Find => "Find",
                        PromptKind::Command => "Ask Minima (e.g. “click sign in”, “search for rust”, “type bob in username”)",
                    };
                    let text: String = p.buf.iter().collect();
                    let before: String = p.buf.iter().take(p.cur).collect();
                    let prefix = format!(" {label}: ");
                    let _ = write!(s, "{}{}[0m {}", if self.cfg.color { "[1;30;46m" } else { "[7m" }, prefix, text);
                    let c = width(&prefix) + 1 + width(&before) + 1;
                    cursor_at = Some((h, c.min(w)));
                }
                Mode::List(l) => {
                    let f: String = l.filter.iter().collect();
                    let _ = write!(s, "\x1b[7m filter: \x1b[0m {}", f);
                    cursor_at = Some((h, 10 + width(&f) + 1));
                }
                _ => {
                    let (text, err) = self.status_line(w);
                    let code = if err { "[1;97;41m" } else if self.cfg.color { "[97;100m" } else { "[7m" };
                    let t = truncate_width(&text, w);
                    let _ = write!(s, "{}{:<w$}\x1b[0m", code, t, w = w);
                }
            }
        }

        if let Some((r, c)) = cursor_at {
            use std::fmt::Write as _;
            let _ = write!(s, "\x1b[{};{}H\x1b[?25h", r, c);
        }
        s.push_str("\x1b[?2026l");
        self.dirty = false;
        s
    }

    // ------------------------------------------------------------------
    // headless testing support (used by `minima --snapshot`)
    // ------------------------------------------------------------------
    pub fn new_headless(cfg: Config, size: (u16, u16)) -> App {
        let mut a = App::new(cfg);
        a.headless = true;
        a.size = size;
        a.out = Box::new(std::io::sink());
        a
    }

    /// Process worker messages until nothing is loading (or `max` elapses).
    pub fn pump(&mut self, max: Duration) {
        let t0 = Instant::now();
        loop {
            while let Ok(m) = self.rx.try_recv() {
                self.handle_msg(m);
            }
            if (self.loading.is_none() && !self.pending_image) || t0.elapsed() > max {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    pub fn test_open(&mut self, input: &str) {
        self.open_input(input);
        self.pump(Duration::from_secs(40));
    }

    pub fn test_key(&mut self, k: KeyEvent) {
        self.handle_event(Event::Key(k));
        self.pump(Duration::from_secs(40));
    }

    pub fn status_text(&self) -> String {
        self.status_line(self.size.0 as usize).0
    }

    fn status_line(&self, w: usize) -> (String, bool) {
        if let Some(u) = &self.loading {
            const SP: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
            return (format!(" {} loading {}   (Esc to cancel)", SP[self.spin % 8], truncate_width(u, w.saturating_sub(30))), false);
        }
        if self.status_at.elapsed() < Duration::from_secs(8) && !self.status.is_empty() {
            return (format!(" {}", self.status), self.status_err);
        }
        let pct = self.lay.as_ref().map_or(0, |l| {
            let total = l.lines.len().saturating_sub(self.view_h()).max(1);
            (self.scroll * 100 / total).min(100)
        });
        if let (Some(wid), Some(d)) = (self.focus, self.doc()) {
            if let Some(wd) = d.widgets.get(wid) {
                let t = match &wd.kind {
                    WidgetKind::Link { href } => format!(" → {href}"),
                    WidgetKind::Image { src } => format!(" image: {}  (Enter/i view as ASCII · d download)  {}", wd.label, truncate_width(src, 50)),
                    WidgetKind::Video { .. } => format!(" video: {}  (Enter/p play · d download)", wd.label),
                    WidgetKind::Text { .. } => format!(" field: {}  (Enter to type · Enter again to submit)", wd.label),
                    WidgetKind::Check { .. } => format!(" option: {}  (Enter toggles)", wd.label),
                    WidgetKind::Select { .. } => format!(" menu: {}  (Enter to choose)", wd.label),
                    WidgetKind::Button { submit, .. } => format!(" button: {}{}", wd.label, if *submit { "  (Enter to submit)" } else { "  (needs JavaScript)" }),
                };
                return (t, false);
            }
        }
        if let Some(d) = self.doc() {
            let t = format!(
                " {} · {} · {} stripped · {}%   Tab select · : ask · g go · / find · ? help",
                truncate_width(&d.title, 40),
                d.kind.name(),
                d.stripped,
                pct
            );
            return (t, false);
        }
        (" Minima — press g to go somewhere, ? for help".into(), false)
    }

    fn sgr(&self, st: Style, focused: bool, editing: bool) -> String {
        let color = self.cfg.color;
        let base: &str = if color {
            match st {
                Style::Normal => "",
                Style::H1 => "\x1b[1;96m",
                Style::H2 => "\x1b[1;36m",
                Style::H3 => "\x1b[1;94m",
                Style::Link => "\x1b[4;36m",
                Style::Image => "\x1b[35m",
                Style::Video => "\x1b[1;35m",
                Style::Button => "\x1b[30;46m",
                Style::Field => "\x1b[1;97;44m",
                Style::Hint => "\x1b[37;44m",
                Style::Check => "\x1b[1;33m",
                Style::Select => "\x1b[33;100m",
                Style::Quote => "\x1b[3;2m",
                Style::Pre => "\x1b[32m",
                Style::Dim => "\x1b[2m",
                Style::Marker => "\x1b[33m",
            }
        } else {
            match st {
                Style::Normal => "",
                Style::H1 | Style::H2 | Style::H3 => "\x1b[1m",
                Style::Link => "\x1b[4m",
                Style::Image | Style::Video => "\x1b[3m",
                Style::Button => "\x1b[7m",
                Style::Field | Style::Select => "\x1b[4m",
                Style::Hint => "\x1b[2;4m",
                Style::Check => "\x1b[1m",
                Style::Quote => "\x1b[3m",
                Style::Pre => "",
                Style::Dim => "\x1b[2m",
                Style::Marker => "",
            }
        };
        if editing {
            return "\x1b[1;30;107m".to_string();
        }
        if focused {
            return format!("{}\x1b[7m", base);
        }
        base.to_string()
    }

    fn draw_list(&self, s: &mut String, l: &ListView, w: usize, vh: usize) {
        use std::fmt::Write as _;
        let vis = l.visible();
        let rows = vh.saturating_sub(2).max(1);
        let sel = l.sel.min(vis.len().saturating_sub(1));
        let top = if sel >= rows { sel + 1 - rows } else { 0 };
        let _ = write!(s, "\x1b[2;1H\x1b[0m\x1b[1;96m {}\x1b[0m\x1b[K", truncate_width(&l.title, w.saturating_sub(2)));
        for r in 0..vh - 1 {
            let _ = write!(s, "\x1b[{};1H\x1b[0m\x1b[K", r + 3);
            let i = top + r;
            if let Some(&idx) = vis.get(i) {
                let it = &l.items[idx];
                let label = truncate_width(&it.label, w * 6 / 10);
                let detail = truncate_width(&it.detail, w.saturating_sub(width(&label) + 6));
                if i == sel {
                    let _ = write!(s, "\x1b[7m {:<w$}\x1b[0m", format!("{label}  {detail}"), w = w.saturating_sub(1));
                } else {
                    let _ = write!(s, " {label}  \x1b[2m{detail}\x1b[0m");
                }
            } else if i == 0 && vis.is_empty() {
                let _ = write!(s, "\x1b[2m  (nothing here)\x1b[0m");
            }
        }
    }

    // ------------------------------------------------------------------
    // main loop
    // ------------------------------------------------------------------
    pub fn run(&mut self, start: Option<String>) {
        self.cfg.ensure_default_file();
        let target = start.unwrap_or_else(|| self.cfg.home.clone());
        // Home: show our own instant page first if the network is slow
        if target == self.cfg.home && target.starts_with("http") {
            self.navigate(&target, None);
        } else if target.starts_with("about:") {
            self.navigate(&target, None);
        } else {
            self.open_input(&target);
        }
        let mut last_tick = Instant::now();
        while !self.quit {
            if self.dirty {
                self.draw();
            }
            if event::poll(Duration::from_millis(40)).unwrap_or(false) {
                // drain a burst of events before redrawing
                loop {
                    match event::read() {
                        Ok(ev) => self.handle_event(ev),
                        Err(_) => break,
                    }
                    if self.quit || !event::poll(Duration::from_millis(0)).unwrap_or(false) {
                        break;
                    }
                }
            }
            while let Ok(m) = self.rx.try_recv() {
                self.handle_msg(m);
            }
            if self.loading.is_some() && last_tick.elapsed() > Duration::from_millis(90) {
                self.spin += 1;
                last_tick = Instant::now();
                self.dirty = true;
            }
            // expire status messages
            if !self.status.is_empty() && self.status_at.elapsed() > Duration::from_secs(8) {
                self.status.clear();
                self.dirty = true;
            }
            // fall back to the built-in home page if the first load failed
            if self.pages.is_empty() && self.loading.is_none() && self.nav_gen > 0 && self.status_err {
                let home = pages::home(&self.cfg);
                let msg = self.status.clone();
                self.install_doc(home);
                self.err(msg);
            }
        }
        if self.font_shrunk {
            let _ = self.out.write_all(b"\x1b]50;#+2\x07");
        }
    }

    pub fn download_dir(&self) -> PathBuf {
        self.cfg.download_dir.clone()
    }
}

fn layout_field_inner_width(lay: &Layout, line: usize, col: usize) -> usize {
    // find the seg starting at `col` on `line` and return its inner width
    let mut c = 0usize;
    for seg in &lay.lines[line].segs {
        let w = width(&seg.text);
        if c == col + 0 || (c <= col && col < c + w) {
            if seg.text.starts_with('[') && seg.text.ends_with(']') {
                return width(&seg.text).saturating_sub(2);
            }
        }
        c += w;
    }
    20
}

fn name_of(p: &std::path::Path) -> String {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string()
}

fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let mut p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    for i in 1..1000 {
        p = dir.join(format!("{stem} ({i}){ext}"));
        if !p.exists() {
            break;
        }
    }
    p
}
