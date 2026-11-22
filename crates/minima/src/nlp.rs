//! The compiled-in "language model": a small, fast, allocation-light set of
//! linear models and lexical tools that understand web pages.
//!
//! * `score_container` — logistic scorer deciding whether a DOM block is
//!   content or chrome (nav, ads, cookie walls...).
//! * `score_form`      — decides whether a form is worth showing, and what it is for.
//! * `classify_page`   — what kind of page this is.
//! * `Matcher`         — fuzzy natural-language matching (stemming, synonyms,
//!   IDF-ish weighting) used by the command bar, find, and "ask the page".
//!
//! All weights and lexicons are baked in at build time from `model/*.txt`;
//! there is no model file to ship and no runtime loading.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/model_data.rs"));

pub struct Model {
    w: HashMap<&'static str, f32>,
    stop: HashSet<&'static str>,
    cls_pos: HashSet<&'static str>,
    cls_neg: HashSet<&'static str>,
    form_pos: HashSet<&'static str>,
    form_neg: HashSet<&'static str>,
    /// stem -> synonym group id
    syn: HashMap<String, Vec<usize>>,
}

pub fn model() -> &'static Model {
    static M: OnceLock<Model> = OnceLock::new();
    M.get_or_init(|| {
        let mut syn: HashMap<String, Vec<usize>> = HashMap::new();
        for (gi, group) in SYNONYMS.iter().enumerate() {
            for phrase in group.iter() {
                for tok in tokenize_raw(phrase) {
                    let e = syn.entry(stem(&tok)).or_default();
                    if !e.contains(&gi) {
                        e.push(gi);
                    }
                }
            }
        }
        Model {
            w: WEIGHTS.iter().cloned().collect(),
            stop: STOPWORDS.iter().cloned().collect(),
            cls_pos: CLASS_POS.iter().cloned().collect(),
            cls_neg: CLASS_NEG.iter().cloned().collect(),
            form_pos: FORM_POS.iter().cloned().collect(),
            form_neg: FORM_NEG.iter().cloned().collect(),
            syn,
        }
    })
}

impl Model {
    fn w(&self, k: &str) -> f32 {
        *self.w.get(k).unwrap_or(&0.0)
    }
}

// ---------------------------------------------------------------------------
// tokenisation
// ---------------------------------------------------------------------------

fn tokenize_raw(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            // split camelCase
            if ch.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = ch.is_lowercase();
            for l in ch.to_lowercase() {
                cur.push(l);
            }
        } else {
            prev_lower = false;
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Very small suffix-stripping stemmer (enough to match "logging"/"login", "images"/"image").
pub fn stem(w: &str) -> String {
    let mut s = w.to_string();
    if s.len() > 4 && s.ends_with("ies") {
        s.truncate(s.len() - 3);
        s.push('y');
        return s;
    }
    if s.len() > 3 && s.ends_with('s') && !s.ends_with("ss") && !s.ends_with("us") && !s.ends_with("is") {
        s.pop();
    }
    for suf in ["ing", "ed", "ly"] {
        if s.len() >= suf.len() + 4 && s.ends_with(suf) {
            s.truncate(s.len() - suf.len());
            break;
        }
    }
    s
}

/// Tokens (lowercased, camelCase-split) of an attribute-like string, e.g. a class list.
pub fn attr_tokens(s: &str) -> Vec<String> {
    tokenize_raw(s)
}

/// Content tokens: stopwords removed, stemmed.
pub fn content_tokens(s: &str) -> Vec<String> {
    let m = model();
    tokenize_raw(s)
        .into_iter()
        .filter(|t| !m.stop.contains(t.as_str()))
        .map(|t| stem(&t))
        .collect()
}

// ---------------------------------------------------------------------------
// container model
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub words: u32,
    pub link_words: u32,
    pub sentences: u32,
    pub paragraphs: u32,
    pub headings: u32,
    pub widgets: u32,
    pub links: u32,
    pub items: u32,
    pub inputs: u32,
    pub password: bool,
    pub depth: u32,
}

impl Stats {
    pub fn add(&mut self, o: &Stats) {
        self.words += o.words;
        self.link_words += o.link_words;
        self.sentences += o.sentences;
        self.paragraphs += o.paragraphs;
        self.headings += o.headings;
        self.widgets += o.widgets;
        self.links += o.links;
        self.items += o.items;
        self.inputs += o.inputs;
        self.password |= o.password;
    }
    pub fn link_density(&self) -> f32 {
        if self.words == 0 {
            0.0
        } else {
            (self.link_words as f32 / self.words as f32).min(1.0)
        }
    }
}

pub struct ContainerInfo<'a> {
    pub tag: &'a str,
    pub role: &'a str,
    pub class_id: &'a str,
    pub stats: Stats,
}

pub fn count_hits(set: &HashSet<&'static str>, tokens: &[String]) -> usize {
    tokens.iter().filter(|t| set.contains(t.as_str())).count()
}

/// How many class/id tokens look like page chrome (ads, banners, menus...).
pub fn neg_hits(tokens: &[String]) -> usize {
    count_hits(&model().cls_neg, tokens)
}

/// Returns the logit; `>= container.threshold` means keep.
pub fn score_container(c: &ContainerInfo) -> f32 {
    let m = model();
    let s = &c.stats;
    let mut z = m.w("container.bias");
    z += m.w(&format!("tag.{}", c.tag));
    if !c.role.is_empty() {
        z += m.w(&format!("role.{}", c.role));
    }
    let toks = attr_tokens(c.class_id);
    let pos = count_hits(&m.cls_pos, &toks).min(2) as f32;
    let neg = count_hits(&m.cls_neg, &toks).min(2) as f32;
    z += m.w("cls.pos") * pos / 2.0_f32.max(1.0) * 1.0;
    z += m.w("cls.neg") * neg / 1.5;
    let words = s.words as f32;
    z += m.w("stat.len_log") * (1.0 + words).ln();
    z += m.w("stat.link_density") * s.link_density();
    let sent = if words > 0.0 { (s.sentences as f32 * 20.0 / words).min(1.0) } else { 0.0 };
    z += m.w("stat.sentence_density") * sent;
    z += m.w("stat.para_log") * (1.0 + s.paragraphs as f32).ln();
    if s.headings > 0 {
        z += m.w("stat.heading");
    }
    if words > 0.0 {
        z += m.w("stat.widget_density") * (s.widgets as f32 * 10.0 / words).min(3.0);
    }
    if s.words == 0 && s.widgets == 0 {
        z += m.w("stat.empty");
    }
    z += m.w("stat.depth") * s.depth as f32;
    z
}

pub fn container_threshold() -> f32 {
    model().w("container.threshold")
}

// ---------------------------------------------------------------------------
// form model
// ---------------------------------------------------------------------------

pub struct FormSignals<'a> {
    pub action: &'a str,
    pub class_id: &'a str,
    pub labels: &'a [String],
    pub has_text: bool,
    pub fields: usize,
    pub password: bool,
}

/// Returns (logit, intent).
pub fn score_form(f: &FormSignals) -> (f32, &'static str) {
    let m = model();
    let mut z = m.w("form.bias");
    let mut toks = attr_tokens(f.action);
    toks.extend(attr_tokens(f.class_id));
    for l in f.labels {
        toks.extend(attr_tokens(l));
    }
    let pos = count_hits(&m.form_pos, &toks).min(3) as f32;
    let neg = count_hits(&m.form_neg, &toks).min(2) as f32;
    z += m.w("form.tok_pos") * pos / 3.0 * 1.5;
    z += m.w("form.tok_neg") * neg / 2.0 * 1.5;
    if f.has_text {
        z += m.w("form.has_text");
    }
    z += m.w("form.many_fields") * f.fields.saturating_sub(4) as f32;
    let has = |w: &str| toks.iter().any(|t| t == w);
    let intent = if f.password {
        "login"
    } else if !f.has_text {
        "form"
    } else if has("search") || has("query") || has("q") || has("find") || has("keyword") || has("keywords") {
        "search"
    } else if has("newsletter") || has("subscribe") {
        "newsletter"
    } else if has("comment") {
        "comment"
    } else {
        "form"
    };
    (z, intent)
}

pub fn form_threshold() -> f32 {
    model().w("form.threshold")
}

// ---------------------------------------------------------------------------
// page kind
// ---------------------------------------------------------------------------

pub struct PageSignals {
    pub stats: Stats,
    pub has_article: bool,
    pub forms: usize,
    pub url_path: String,
    pub url_query: String,
}

pub fn classify_page(p: &PageSignals) -> crate::doc::PageKind {
    use crate::doc::PageKind as K;
    let m = model();
    let s = &p.stats;
    let words_log = (1.0 + s.words as f32).ln();
    let para_log = (1.0 + s.paragraphs as f32).ln();
    let ld = s.link_density();
    let few = if s.words < 80 { 1.0 } else { 0.0 };
    let has_form = if p.forms > 0 { 1.0 } else { 0.0 };
    let path = p.url_path.to_lowercase();
    let url_search = if path.contains("search") || p.url_query.contains("q=") || p.url_query.contains("query=") {
        1.0
    } else {
        0.0
    };
    let url_login = if path.contains("login") || path.contains("signin") || path.contains("sign-in") || path.contains("auth") {
        1.0
    } else {
        0.0
    };
    let url_root = if (path == "/" || path.is_empty()) && p.url_query.is_empty() { 1.0 } else { 0.0 };

    let scores = [
        (
            K::Article,
            m.w("kind.article.bias")
                + m.w("kind.article.words_log") * words_log
                + m.w("kind.article.para_log") * para_log
                + m.w("kind.article.link_density") * ld
                + m.w("kind.article.has_article") * if p.has_article { 1.0 } else { 0.0 },
        ),
        (
            K::Results,
            m.w("kind.results.bias")
                + m.w("kind.results.link_density") * ld
                + m.w("kind.results.links_log") * (1.0 + s.links as f32).ln()
                + m.w("kind.results.url_search") * url_search
                + m.w("kind.results.has_form") * has_form,
        ),
        (
            K::Login,
            m.w("kind.login.bias")
                + m.w("kind.login.password") * if s.password { 1.0 } else { 0.0 }
                + m.w("kind.login.url_login") * url_login
                + m.w("kind.login.few_words") * few,
        ),
        (
            K::Form,
            m.w("kind.form.bias")
                + m.w("kind.form.inputs_log") * (1.0 + s.inputs as f32).ln()
                + m.w("kind.form.few_words") * few,
        ),
        (
            K::Listing,
            m.w("kind.listing.bias")
                + m.w("kind.listing.link_density") * ld
                + m.w("kind.listing.items_log") * (1.0 + s.items as f32).ln(),
        ),
        (
            K::Home,
            m.w("kind.home.bias")
                + m.w("kind.home.few_words") * few
                + m.w("kind.home.has_form") * has_form
                + m.w("kind.home.url_root") * url_root,
        ),
    ];
    let mut best = (K::Other, 0.0f32);
    for (k, v) in scores {
        if v > best.1 {
            best = (k, v);
        }
    }
    best.0
}

// ---------------------------------------------------------------------------
// fuzzy natural-language matching
// ---------------------------------------------------------------------------

fn norm(s: &str) -> String {
    tokenize_raw(s).join(" ")
}

/// Expanded query: stems plus synonym expansions (single words and whole phrases).
pub struct Matcher {
    terms: Vec<(String, f32)>,
    phrases: Vec<(String, f32)>,
    raw: String,
    primary: usize,
}

impl Matcher {
    pub fn new(query: &str) -> Matcher {
        let m = model();
        let raw = norm(query);
        let toks = content_tokens(query);
        let mut terms: Vec<(String, f32)> = Vec::new();
        let mut phrases: Vec<(String, f32)> = Vec::new();
        fn push(terms: &mut Vec<(String, f32)>, t: String, w: f32) {
            if let Some(e) = terms.iter_mut().find(|e| e.0 == t) {
                if w > e.1 {
                    e.1 = w;
                }
            } else {
                terms.push((t, w));
            }
        }
        for t in &toks {
            push(&mut terms, t.clone(), 1.0);
        }
        let primary = toks.len();

        // Whole-query synonym groups ("sign in" ~ "log in" ~ "login").
        let mut whole_hit = false;
        for group in SYNONYMS.iter() {
            if group.iter().any(|p| norm(p) == raw) {
                whole_hit = true;
                for p in group.iter() {
                    let np = norm(p);
                    if np != raw {
                        phrases.push((np.clone(), 1.5));
                    }
                    for tok in tokenize_raw(p) {
                        if !m.stop.contains(tok.as_str()) {
                            push(&mut terms, stem(&tok), 0.4);
                        }
                    }
                }
            }
        }
        // Single-word queries expand through every group they belong to.
        if !whole_hit && toks.len() == 1 {
            if let Some(groups) = m.syn.get(&toks[0]) {
                for g in groups {
                    for p in SYNONYMS[*g].iter() {
                        let np = norm(p);
                        if np.contains(' ') {
                            phrases.push((np, 0.8));
                        } else if !m.stop.contains(np.as_str()) {
                            push(&mut terms, stem(&np), 0.55);
                        }
                    }
                }
            }
        }
        phrases.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.partial_cmp(&a.1).unwrap()));
        phrases.dedup_by(|a, b| a.0 == b.0);
        Matcher { terms, phrases, raw, primary }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Lenient score, for free-text relevance ("ask the page"). 0 means no overlap.
    pub fn score(&self, text: &str) -> f32 {
        self.score_with(text, false)
    }

    /// Strict score, for UI labels: a multi-word query such as "sign in" must match
    /// as a phrase (or via its synonyms), not just share the word "sign".
    pub fn score_label(&self, text: &str) -> f32 {
        self.score_with(text, true)
    }

    fn score_with(&self, text: &str, strict: bool) -> f32 {
        if self.terms.is_empty() {
            return 0.0;
        }
        let ntext = norm(text);
        if ntext.is_empty() {
            return 0.0;
        }
        let toks: Vec<String> = ntext.split(' ').map(stem).collect();
        let mut sum = 0.0;
        let mut hit_primary = 0.0;
        for (t, w) in &self.terms {
            let tf = toks.iter().filter(|x| *x == t).count() as f32;
            if tf > 0.0 {
                let sat = tf * 2.2 / (tf + 1.2);
                let rarity = 0.6 + (t.len() as f32).min(10.0) / 10.0;
                sum += w * sat * rarity;
                if *w >= 1.0 {
                    hit_primary += 1.0;
                }
            } else if *w >= 1.0 && t.len() >= 4 && !strict {
                // prefix / typo tolerance
                if toks.iter().any(|x| (x.len() >= 4) && (x.starts_with(t.as_str()) || t.starts_with(x.as_str()))) {
                    sum += w * 0.6;
                    hit_primary += 0.6;
                }
            }
        }
        let padded = format!(" {} ", ntext);
        let mut phrase_hit = false;
        if !self.raw.is_empty() && padded.contains(&format!(" {} ", self.raw)) {
            sum += 8.0;
            phrase_hit = true;
        }
        for (p, w) in &self.phrases {
            if padded.contains(&format!(" {} ", p)) {
                sum += w * 2.0;
                phrase_hit = true;
            }
        }
        if sum == 0.0 {
            return 0.0;
        }
        if strict && self.primary >= 2 && !phrase_hit && hit_primary / (self.primary as f32) < 0.6 {
            return 0.0;
        }
        // short, precise labels beat long paragraphs
        let norm_len = 1.0 / (1.0 + (toks.len() as f32 / 12.0));
        sum * (0.4 + 0.6 * norm_len) + hit_primary * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems() {
        assert_eq!(stem("images"), "image");
        assert_eq!(stem("class"), "class");
        assert_eq!(stem("logging"), "logg");
    }

    #[test]
    fn synonyms_match() {
        let m = Matcher::new("sign in");
        assert!(m.score_label("Log in") > 0.0);
        assert!(m.score_label("Login to your account") > 0.0);
        assert_eq!(m.score_label("Sign up"), 0.0, "sign up must not satisfy sign in");
        assert!(m.score_label("Sign in") > m.score_label("Log in"));
        let m = Matcher::new("buy");
        assert!(m.score_label("Add to cart") > 0.0);
        let m = Matcher::new("contact");
        assert!(m.score_label("Support") > 0.0);
    }

    #[test]
    fn relevance_ranking() {
        let m = Matcher::new("shipping cost");
        let a = m.score("Shipping costs depend on the weight of the parcel.");
        let b = m.score("Our team loves hiking and photography.");
        assert!(a > b);
    }

    #[test]
    fn nav_is_dropped_article_kept() {
        let nav = ContainerInfo {
            tag: "nav",
            role: "",
            class_id: "main-menu",
            stats: Stats { words: 30, link_words: 28, links: 12, ..Default::default() },
        };
        let art = ContainerInfo {
            tag: "div",
            role: "",
            class_id: "post-content",
            stats: Stats { words: 400, link_words: 20, sentences: 22, paragraphs: 6, headings: 1, ..Default::default() },
        };
        assert!(score_container(&nav) < container_threshold());
        assert!(score_container(&art) >= container_threshold());
    }

    #[test]
    fn form_intent() {
        let labels = vec!["Search".to_string()];
        let (z, i) = score_form(&FormSignals {
            action: "/search",
            class_id: "",
            labels: &labels,
            has_text: true,
            fields: 1,
            password: false,
        });
        assert!(z >= form_threshold());
        assert_eq!(i, "search");
    }
}

