//! Networking: a shared HTTP agent with cookies, redirects and compression.

use std::io::Read;
use std::time::Duration;

use url::Url;

pub struct Fetched {
    pub url: Url,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

#[derive(Clone)]
pub struct Fetcher {
    agent: ureq::Agent,
}

const MAX_BODY: u64 = 64 * 1024 * 1024;

impl Fetcher {
    pub fn new(user_agent: &str) -> Fetcher {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(user_agent)
            .timeout_global(Some(Duration::from_secs(45)))
            .timeout_connect(Some(Duration::from_secs(12)))
            .http_status_as_error(false)
            .max_redirects(10)
            .build()
            .into();
        Fetcher { agent }
    }

    pub fn get(&self, url: &Url) -> Result<Fetched, String> {
        let resp = self
            .agent
            .get(url.as_str())
            .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9")
            .call()
            .map_err(|e| e.to_string())?;
        Self::finish(url, resp)
    }

    pub fn post_form(&self, url: &Url, fields: &[(String, String)]) -> Result<Fetched, String> {
        let body = encode_pairs(fields);
        let resp = self
            .agent
            .post(url.as_str())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
            .send(body.as_bytes())
            .map_err(|e| e.to_string())?;
        Self::finish(url, resp)
    }

    fn finish(orig: &Url, mut resp: ureq::http::Response<ureq::Body>) -> Result<Fetched, String> {
        use ureq::ResponseExt;
        let status = resp.status().as_u16();
        let final_url = Url::parse(&resp.get_uri().to_string()).unwrap_or_else(|_| orig.clone());
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut body = Vec::new();
        resp.body_mut()
            .as_reader()
            .take(MAX_BODY)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        Ok(Fetched { url: final_url, status, content_type, body })
    }

    /// Stream to a file, reporting progress. Returns total bytes.
    pub fn download(&self, url: &Url, path: &std::path::Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<u64, String> {
        let resp = self.agent.get(url.as_str()).call().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let mut body = resp.into_body();
        let mut reader = body.as_reader();
        let tmp = path.with_extension("part");
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        let mut last = std::time::Instant::now();
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut f, &buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if last.elapsed() > Duration::from_millis(150) {
                progress(done, total);
                last = std::time::Instant::now();
            }
        }
        drop(f);
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        progress(done, total);
        Ok(done)
    }
}

pub fn encode_pairs(fields: &[(String, String)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", crate::util::url_encode(k), crate::util::url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Decode a response body to UTF-8 using the Content-Type charset, a BOM, or `<meta charset>`.
pub fn decode_html(body: &[u8], content_type: &str) -> String {
    let mut label: Option<String> = None;
    if let Some(i) = content_type.to_lowercase().find("charset=") {
        let v = &content_type[i + 8..];
        let v = v.split(';').next().unwrap_or("").trim().trim_matches('"');
        label = Some(v.to_string());
    }
    if label.is_none() {
        let head = &body[..body.len().min(4096)];
        let head_s = String::from_utf8_lossy(head).to_lowercase();
        if let Some(i) = head_s.find("charset=") {
            let rest = &head_s[i + 8..];
            let v: String = rest.trim_start_matches(['"', '\'']).chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            if !v.is_empty() {
                label = Some(v);
            }
        }
    }
    let enc = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (s, _, _) = enc.decode(body);
    s.into_owned()
}

pub fn query_pairs_url(base: &Url, fields: &[(String, String)]) -> Url {
    let mut u = base.clone();
    u.set_fragment(None);
    let q = encode_pairs(fields);
    // GET forms replace the query string
    u.set_query(if q.is_empty() { None } else { Some(&q) });
    u
}

/// Turn low-level network errors into something a person can act on.
pub fn friendly_error(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("no such host") || l.contains("failed to lookup") || l.contains("dns") || l.contains("name or service not known") || l.contains("nodename nor servname") {
        "can't find that site (DNS lookup failed). Check the address and your connection".into()
    } else if l.contains("timed out") || l.contains("timeout") {
        "the site took too long to respond".into()
    } else if l.contains("refused") {
        "the site refused the connection".into()
    } else if l.contains("certificate") || l.contains("tls") || l.contains("handshake") {
        format!("secure connection failed ({e})")
    } else if l.contains("too many redirects") || l.contains("redirect") {
        "the site redirected too many times".into()
    } else if l.contains("network is unreachable") || l.contains("connection reset") || l.contains("connect") {
        format!("could not connect ({e}). Are you online?")
    } else {
        e.to_string()
    }
}
