//! Web access for the utility agent: Tavily search and a page reader.
//!
//! These belong to the utility agent. Other agents don't get them directly:
//! when a planned job needs web data, the planner asks the utility agent to
//! search and passes the result on to the other agents as data.
//!
//! Everything returned here is written by third parties, so callers treat it
//! as untrusted. The page reader refuses addresses on the local network, since
//! a model that has been misled by a page must not be able to probe the user's
//! own machine or LAN.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use reqwest::{redirect, Url};
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value};

const SEARCH_URL: &str = "https://api.tavily.com/search";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";
const TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_RESULTS: usize = 5;
const MAX_RESULTS: usize = 8;
const MAX_PAGE_BYTES: usize = 1_500_000;
const MAX_PAGE_CHARS: usize = 6000;
const MAX_REDIRECTS: usize = 3;

/// A client that never follows redirects by itself, so every hop can be
/// checked against the local-network rule first.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(redirect::Policy::none())
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .build()
            .expect("http client")
    })
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
    /// "general" (default) or "news".
    #[serde(default)]
    topic: Option<String>,
}

#[derive(Deserialize)]
struct FetchArgs {
    url: String,
}

// ---------- search ----------

/// The Tavily key, handed over from Settings. Search is only offered to the
/// model while one is set.
#[derive(Default)]
pub struct WebState {
    api_key: Mutex<String>,
}

impl WebState {
    pub fn key(&self) -> Option<String> {
        let key = self.api_key.lock().unwrap().clone();
        (!key.is_empty()).then_some(key)
    }
}

#[tauri::command]
pub fn web_configure(state: tauri::State<'_, WebState>, api_key: String) {
    *state.api_key.lock().unwrap() = api_key.trim().to_string();
}

#[tauri::command]
pub fn web_is_configured(state: tauri::State<'_, WebState>) -> bool {
    state.key().is_some()
}

pub async fn search(api_key: &str, args: &str) -> Result<String, String> {
    let SearchArgs { query, max_results, topic } =
        serde_json::from_str(args).map_err(|e| format!("Bad arguments: {e}"))?;
    let query = query.trim();
    if query.is_empty() {
        return Err("The search query is empty.".to_string());
    }
    let limit = max_results.unwrap_or(DEFAULT_RESULTS).clamp(1, MAX_RESULTS);
    let topic = if topic.as_deref() == Some("news") { "news" } else { "general" };

    let resp = client()
        .post(SEARCH_URL)
        .bearer_auth(api_key)
        .json(&json!({
            "query": query,
            "max_results": limit,
            "search_depth": "advanced",
            "include_answer": "basic",
            "topic": topic,
        }))
        .send()
        .await
        .map_err(|e| format!("Search failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "The web search key was rejected. Check it in settings.".to_string(),
            429 => "Web search is rate limited right now. Try again in a moment.".to_string(),
            432 | 433 => "The web search plan's limit has been reached.".to_string(),
            code => format!("Search failed (status {code})."),
        });
    }

    let value: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(format_results(&value, query))
}

const MAX_SNIPPET_CHARS: usize = 900;

fn format_results(value: &Value, query: &str) -> String {
    let results = value["results"].as_array().cloned().unwrap_or_default();
    let answer = value["answer"].as_str().unwrap_or("").trim();
    if results.is_empty() && answer.is_empty() {
        return format!("No results found for \"{query}\".");
    }

    let mut out = format!("Web results for \"{query}\":\n");
    if !answer.is_empty() {
        out.push_str(&format!("Summary: {answer}\n"));
    }
    for (i, r) in results.iter().enumerate() {
        let title = r["title"].as_str().unwrap_or("Untitled").trim();
        let url = r["url"].as_str().unwrap_or("");
        out.push_str(&format!("{}. {title} - {url}\n", i + 1));
        let snippet: String = clean_text(r["content"].as_str().unwrap_or(""))
            .chars()
            .take(MAX_SNIPPET_CHARS)
            .collect();
        if !snippet.is_empty() {
            out.push_str(&format!("   {snippet}\n"));
        }
    }
    out
}

// ---------- page reader ----------

pub async fn fetch(args: &str) -> Result<String, String> {
    let FetchArgs { url } = serde_json::from_str(args).map_err(|e| format!("Bad arguments: {e}"))?;
    let mut current = Url::parse(url.trim()).map_err(|_| "That isn't a valid URL.".to_string())?;

    for _ in 0..=MAX_REDIRECTS {
        ensure_public(&current).await?;
        let mut resp = client()
            .get(current.clone())
            .send()
            .await
            .map_err(|e| format!("Couldn't load the page: {e}"))?;

        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("The page redirected without saying where.")?;
            current = current.join(location).map_err(|_| "Bad redirect.".to_string())?;
            continue;
        }
        if !resp.status().is_success() {
            return Err(format!("The page answered {}.", resp.status()));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let readable = content_type.is_empty()
            || content_type.contains("text/")
            || content_type.contains("xml")
            || content_type.contains("json");
        if !readable {
            return Err(format!("Can't read this kind of content ({content_type})."));
        }

        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_PAGE_BYTES {
                break;
            }
        }
        let body = String::from_utf8_lossy(&bytes).to_string();
        return Ok(page_to_text(&body, &current, content_type.contains("html") || content_type.is_empty()));
    }
    Err("Too many redirects.".to_string())
}

fn page_to_text(body: &str, url: &Url, is_html: bool) -> String {
    let (title, text) = if is_html {
        let title_re = Regex::new(r"(?s)<title[^>]*>(.*?)</title>").unwrap();
        let title = title_re
            .captures(body)
            .map(|c| clean_text(&c[1]))
            .unwrap_or_default();
        let mut stripped = body.to_string();
        for tag in ["script", "style", "noscript", "svg", "head"] {
            let re = Regex::new(&format!(r"(?is)<{tag}[\s>].*?</{tag}>")).unwrap();
            stripped = re.replace_all(&stripped, " ").to_string();
        }
        // Keep paragraph breaks readable before dropping the remaining tags.
        let breaks = Regex::new(r"(?i)</(p|div|li|h[1-6]|tr|section|article)>|<br\s*/?>").unwrap();
        stripped = breaks.replace_all(&stripped, "\n").to_string();
        (title, clean_text_keep_lines(&stripped))
    } else {
        (String::new(), body.trim().to_string())
    };

    let mut text: String = text.chars().take(MAX_PAGE_CHARS).collect();
    if text.chars().count() >= MAX_PAGE_CHARS {
        text.push_str("\n[truncated]");
    }
    if title.is_empty() {
        format!("Page: {url}\n\n{text}")
    } else {
        format!("Page: {title} ({url})\n\n{text}")
    }
}

/// Refuses anything that points at this machine or the local network.
async fn ensure_public(url: &Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https pages can be read.".to_string());
    }
    let host = url.host_str().ok_or("That URL has no host.")?.to_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".internal") {
        return Err(local_error());
    }

    // Pages inside the user's Google account need their sign-in; a plain web
    // request just gets a 401. Say so, instead of leaving the agent guessing.
    const GOOGLE_ACCOUNT_HOSTS: [&str; 7] = [
        "docs.google.com",
        "drive.google.com",
        "mail.google.com",
        "calendar.google.com",
        "sheets.google.com",
        "slides.google.com",
        "contacts.google.com",
    ];
    if GOOGLE_ACCOUNT_HOSTS.contains(&host.as_str()) {
        return Err("That is a page inside the user's Google account, which this tool cannot open (it is not signed in). Use the Google agent for Google documents, files and mail.".to_string());
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| "Couldn't find that site.".to_string())?
            .map(|a| a.ip())
            .collect(),
    };
    if addrs.is_empty() || addrs.iter().any(|ip| !is_public_ip(ip)) {
        return Err(local_error());
    }
    Ok(())
}

fn local_error() -> String {
    "That address is on this computer or a private network, so it can't be opened.".to_string()
}

fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: &Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || (a == 100 && (64..=127).contains(&b)) // carrier-grade NAT
        || a == 0)
}

fn is_public_v6(ip: &Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(&v4);
    }
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local
        || (first & 0xffc0) == 0xfe80) // link local
}

// ---------- text helpers ----------

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn strip_tags(html: &str) -> String {
    let tag_re = Regex::new(r"<[^>]+>").unwrap();
    tag_re.replace_all(html, "").to_string()
}

/// Tags removed, entities decoded, all whitespace collapsed to single spaces.
fn clean_text(html: &str) -> String {
    decode_entities(&strip_tags(html))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Same, but keeps line breaks (collapsing runs of blank lines).
fn clean_text_keep_lines(html: &str) -> String {
    let text = decode_entities(&strip_tags(html));
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_local_addresses() {
        assert!(!is_public_ip(&"127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip(&"192.168.1.5".parse().unwrap()));
        assert!(!is_public_ip(&"10.0.0.1".parse().unwrap()));
        assert!(!is_public_ip(&"172.16.4.4".parse().unwrap()));
        assert!(!is_public_ip(&"169.254.169.254".parse().unwrap()));
        assert!(!is_public_ip(&"100.64.0.1".parse().unwrap()));
        assert!(!is_public_ip(&"::1".parse().unwrap()));
        assert!(!is_public_ip(&"fd00::1".parse().unwrap()));
        assert!(!is_public_ip(&"::ffff:10.0.0.1".parse().unwrap()));
        assert!(is_public_ip(&"8.8.8.8".parse().unwrap()));
        assert!(is_public_ip(&"2606:4700::1111".parse().unwrap()));
    }

    #[test]
    fn formats_tavily_results() {
        let value = json!({
            "answer": "Diwali is the festival of lights.",
            "results": [
                { "title": "Diwali - Wikipedia", "url": "https://en.wikipedia.org/wiki/Diwali", "content": "A major   Hindu\nfestival." },
                { "title": "Britannica", "url": "https://www.britannica.com/topic/Diwali-Hindu-festival", "content": "" }
            ]
        });
        let text = format_results(&value, "diwali");
        assert!(text.contains("Summary: Diwali is the festival of lights."));
        assert!(text.contains("1. Diwali - Wikipedia - https://en.wikipedia.org/wiki/Diwali"));
        assert!(text.contains("   A major Hindu festival."));
        assert!(text.contains("2. Britannica"));
        assert_eq!(format_results(&json!({ "results": [] }), "x"), "No results found for \"x\".");
    }

    #[test]
    fn rejects_local_urls() {
        tauri::async_runtime::block_on(async {
            assert!(ensure_public(&Url::parse("http://localhost:1420/").unwrap()).await.is_err());
            assert!(ensure_public(&Url::parse("http://127.0.0.1/").unwrap()).await.is_err());
            assert!(ensure_public(&Url::parse("http://192.168.0.1/admin").unwrap()).await.is_err());
            assert!(ensure_public(&Url::parse("file:///c:/windows/win.ini").unwrap()).await.is_err());
            let google = ensure_public(&Url::parse("https://docs.google.com/document/d/abc/edit").unwrap()).await;
            assert!(google.unwrap_err().contains("Google account"));
        });
    }

    /// Live check: `TAVILY_KEY=... cargo test live_search -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_search() {
        let key = std::env::var("TAVILY_KEY").expect("set TAVILY_KEY");
        tauri::async_runtime::block_on(async {
            let out = search(&key, r#"{"query":"Diwali festival India","max_results":3}"#).await;
            println!("{out:?}");
            assert!(out.is_ok());
            let page = fetch(r#"{"url":"https://example.com"}"#).await;
            println!("{page:?}");
            assert!(page.is_ok());
        });
    }
}
