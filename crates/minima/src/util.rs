//! Small shared helpers.

use unicode_width::UnicodeWidthStr;
use url::Url;

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

pub fn truncate_width(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// Turn whatever the user typed into a URL: a real URL, a bare domain, or a search.
pub fn resolve_input(input: &str, search_template: &str) -> String {
    let t = input.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.starts_with("about:") || t.starts_with("file:") {
        return t.to_string();
    }
    if let Some(rest) = t.strip_prefix("//") {
        return format!("https://{rest}");
    }
    let has_scheme = t.find("://").map(|i| t[..i].chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')).unwrap_or(false);
    if has_scheme {
        return t.to_string();
    }
    let looks_host = !t.contains(' ')
        && (t.starts_with("localhost")
            || t.parse::<std::net::IpAddr>().is_ok()
            || t.split('/').next().map(|h| h.contains('.') && !h.ends_with('.') && h.rsplit('.').next().map(|tld| tld.len() >= 2 && tld.chars().next().map_or(false, |c| c.is_alphabetic() || c.is_ascii_digit())).unwrap_or(false) || h.contains(':')).unwrap_or(false));
    if looks_host {
        let scheme = if t.starts_with("localhost") || t.starts_with("127.") { "http" } else { "https" };
        return format!("{scheme}://{t}");
    }
    search_url(search_template, t)
}

pub fn search_url(template: &str, q: &str) -> String {
    template.replace("{q}", &url_encode(q))
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Unwrap search-engine tracking redirects so links point at the real target.
pub fn unwrap_redirect(href: &str) -> String {
    if let Ok(u) = Url::parse(href) {
        let host = u.host_str().unwrap_or("");
        let path = u.path();
        let pick = |key: &str| u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.to_string());
        if host.contains("google.") && path == "/url" {
            if let Some(v) = pick("q").or_else(|| pick("url")) {
                if v.starts_with("http") {
                    return v;
                }
            }
        }
        if host.ends_with("duckduckgo.com") && path.starts_with("/l/") {
            if let Some(v) = pick("uddg") {
                return v;
            }
        }
        if host.contains("bing.com") && path.starts_with("/ck/") {
            if let Some(v) = pick("u") {
                if let Some(rest) = v.strip_prefix("a1") {
                    if let Some(d) = b64_decode(rest) {
                        if d.starts_with("http") {
                            return d;
                        }
                    }
                }
            }
        }
    }
    href.to_string()
}

fn b64_decode(s: &str) -> Option<String> {
    let mut buf: u32 = 0;
    let mut bits = 0;
    let mut out = Vec::new();
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    String::from_utf8(out).ok()
}

/// Make a safe filename from a title / URL.
pub fn sanitize_filename(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches(|c| c == '_' || c == '.').to_string();
    if out.is_empty() {
        "download".into()
    } else if out.len() > 100 {
        out.chars().take(100).collect()
    } else {
        out
    }
}

pub fn filename_from_url(u: &Url) -> String {
    let seg = u.path_segments().and_then(|s| s.last()).unwrap_or("");
    if seg.is_empty() {
        sanitize_filename(u.host_str().unwrap_or("download"))
    } else {
        sanitize_filename(&percent_decode(seg))
    }
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn human_bytes(n: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Find an executable on PATH, or next to our own binary (bundled helpers).
pub fn find_exe(name: &str) -> Option<std::path::PathBuf> {
    let exe_name = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            for sub in ["", "bin", "../libexec/minima", "../lib/minima", "tools"] {
                let p = dir.join(sub).join(&exe_name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(&exe_name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves() {
        let t = "https://www.google.com/search?q={q}";
        assert_eq!(resolve_input("example.com", t), "https://example.com");
        assert_eq!(resolve_input("rust lang", t), "https://www.google.com/search?q=rust+lang");
        assert_eq!(resolve_input("localhost:8080", t), "http://localhost:8080");
        assert_eq!(resolve_input("https://a.b/c", t), "https://a.b/c");
        assert_eq!(resolve_input("hello", t), "https://www.google.com/search?q=hello");
    }

    #[test]
    fn unwraps() {
        assert_eq!(unwrap_redirect("https://www.google.com/url?q=https%3A%2F%2Frust-lang.org%2F&sa=U"), "https://rust-lang.org/");
        assert_eq!(unwrap_redirect("https://duckduckgo.com/l/?uddg=https%3A%2F%2Fa.com%2Fx"), "https://a.com/x");
    }

    #[test]
    fn decode() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
