//! Persistent browsing history and bookmarks (plain TSV files).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Entry {
    pub time: u64,
    pub url: String,
    pub title: String,
}

pub struct Store {
    path: PathBuf,
    pub entries: Vec<Entry>,
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Store {
    pub fn load(path: PathBuf) -> Store {
        let mut entries = Vec::new();
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let mut it = line.splitn(3, '\t');
                if let (Some(t), Some(u), title) = (it.next(), it.next(), it.next()) {
                    entries.push(Entry { time: t.parse().unwrap_or(0), url: u.to_string(), title: title.unwrap_or("").to_string() });
                }
            }
        }
        Store { path, entries }
    }

    pub fn add(&mut self, url: &str, title: &str) {
        let e = Entry { time: now(), url: url.to_string(), title: clean(title) };
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{}\t{}\t{}", e.time, e.url, e.title);
        }
        self.entries.push(e);
    }

    /// Update the title of the most recent entry for `url` (titles arrive after navigation starts).
    pub fn contains(&self, url: &str) -> bool {
        self.entries.iter().any(|e| e.url == url)
    }

    pub fn remove(&mut self, url: &str) {
        self.entries.retain(|e| e.url != url);
        self.rewrite();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.rewrite();
    }

    fn rewrite(&self) {
        let mut s = String::new();
        for e in &self.entries {
            s.push_str(&format!("{}\t{}\t{}\n", e.time, e.url, e.title));
        }
        let _ = fs::write(&self.path, s);
    }

    /// Most-recent-first, de-duplicated by URL.
    pub fn recent(&self) -> Vec<Entry> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for e in self.entries.iter().rev() {
            if seen.insert(e.url.clone()) {
                out.push(e.clone());
            }
        }
        out
    }
}
