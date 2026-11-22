//! The stripped-down document model that every web page is reduced to.

use url::Url;

#[derive(Clone, Debug)]
pub enum Inline {
    Text(String),
    Widget(usize),
    Break,
}

#[derive(Clone, Debug)]
pub enum Block {
    Heading { level: u8, inl: Vec<Inline> },
    Para(Vec<Inline>),
    Item { depth: u8, marker: String, inl: Vec<Inline> },
    Quote { depth: u8, inl: Vec<Inline> },
    Pre(String),
    Row(Vec<Vec<Inline>>),
    Rule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Clone, Debug)]
pub struct FormInfo {
    pub action: String,
    pub method: Method,
    pub hidden: Vec<(String, String)>,
    /// Rough intent guess ("search", "login", ...), filled in by the NLP pass.
    pub intent: String,
}

#[derive(Clone, Debug)]
pub enum WidgetKind {
    Link { href: String },
    Image { src: String },
    Video { src: String },
    Text { form: Option<usize>, name: String, value: String, password: bool, multiline: bool },
    Check { form: Option<usize>, name: String, value: String, checked: bool, radio: bool },
    Select { form: Option<usize>, name: String, options: Vec<(String, String)>, selected: usize },
    Button { form: Option<usize>, name: String, value: String, submit: bool },
}

#[derive(Clone, Debug)]
pub struct Widget {
    pub kind: WidgetKind,
    /// Human-readable label inferred for this element.
    pub label: String,
    /// Cursor position (in chars) for text fields being edited.
    pub cursor: usize,
    /// Show the label as a hint inside an empty text field.
    pub hint: bool,
}

impl Widget {
    pub fn new(kind: WidgetKind, label: String) -> Self {
        Widget { kind, label, cursor: 0, hint: true }
    }
    pub fn is_text(&self) -> bool {
        matches!(self.kind, WidgetKind::Text { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    Article,
    Results,
    Login,
    Form,
    Listing,
    Home,
    Other,
}

impl PageKind {
    pub fn name(self) -> &'static str {
        match self {
            PageKind::Article => "article",
            PageKind::Results => "results",
            PageKind::Login => "login",
            PageKind::Form => "form",
            PageKind::Listing => "listing",
            PageKind::Home => "home",
            PageKind::Other => "page",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub url: Url,
    pub title: String,
    pub blocks: Vec<Block>,
    pub widgets: Vec<Widget>,
    pub forms: Vec<FormInfo>,
    pub kind: PageKind,
    /// How many DOM blocks were removed as noise (shown in the status bar).
    pub stripped: usize,
    pub raw_bytes: usize,
}

impl Doc {
    pub fn empty(url: Url) -> Doc {
        Doc {
            url,
            title: String::new(),
            blocks: Vec::new(),
            widgets: Vec::new(),
            forms: Vec::new(),
            kind: PageKind::Other,
            stripped: 0,
            raw_bytes: 0,
        }
    }

    /// Plain text of an inline run (widgets contribute their label).
    pub fn inline_text(&self, inl: &[Inline]) -> String {
        let mut s = String::new();
        for i in inl {
            match i {
                Inline::Text(t) => s.push_str(t),
                Inline::Widget(w) => s.push_str(&self.widgets[*w].label),
                Inline::Break => s.push(' '),
            }
        }
        s
    }

    /// Every block as plain text, for search / relevance.
    pub fn block_text(&self, b: &Block) -> String {
        match b {
            Block::Heading { inl, .. } | Block::Para(inl) | Block::Item { inl, .. } | Block::Quote { inl, .. } => {
                self.inline_text(inl)
            }
            Block::Pre(s) => s.clone(),
            Block::Row(cells) => cells.iter().map(|c| self.inline_text(c)).collect::<Vec<_>>().join(" "),
            Block::Rule => String::new(),
        }
    }

    /// Build a tiny synthetic document (used for about: pages).
    pub fn builder(url: Url, title: &str) -> Doc {
        let mut d = Doc::empty(url);
        d.title = title.to_string();
        d
    }

    pub fn push_widget(&mut self, w: Widget) -> usize {
        self.widgets.push(w);
        self.widgets.len() - 1
    }
}
