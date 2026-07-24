//! Port of `flows/tools/search.py` — web search. DuckDuckGo HTML endpoint
//! (no API key). The result parser is pure/testable; the fetch is a thin
//! reqwest call.

use serde_json::{json, Value};

use flows_core::models::WebSearchConfig;

use crate::error::{ToolError, ToolResult};
use crate::html::strip_html;

/// DuckDuckGo wraps result links as `/l/?uddg=<percent-encoded-url>`; unwrap them.
fn unwrap_href(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let enc = &href[idx + 5..];
        let enc = enc.split('&').next().unwrap_or(enc);
        return percent_decode(enc);
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse DuckDuckGo HTML results into `{title, url, snippet}` objects.
pub fn parse_ddg_html(html: &str, max_results: usize) -> Vec<Value> {
    let mut results = Vec::new();
    // Each result: an <a class="result__a" href="...">title</a>, optionally
    // followed by an <a class="result__snippet">...</a>.
    for chunk in html.split("result__a").skip(1) {
        if results.len() >= max_results {
            break;
        }
        let Some(href) = extract_attr(chunk, "href=\"") else {
            continue;
        };
        let Some(title_html) = extract_between(chunk, ">", "</a>") else {
            continue;
        };
        let snippet = chunk
            .find("result__snippet")
            .and_then(|i| extract_between(&chunk[i..], ">", "</a>"))
            .map(|s| strip_html(&s))
            .unwrap_or_default();
        results.push(json!({
            "title": strip_html(&title_html),
            "url": unwrap_href(&href),
            "snippet": snippet,
        }));
    }
    results
}

fn extract_attr(s: &str, key: &str) -> Option<String> {
    let start = s.find(key)? + key.len();
    let end = s[start..].find('"')? + start;
    Some(s[start..end].to_string())
}

fn extract_between(s: &str, open: &str, close: &str) -> Option<String> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)? + start;
    Some(s[start..end].to_string())
}

/// Run a web search. Backends: `duckduckgo` (default, HTML scrape, no config) and
/// `searxng` (a self-hosted/private SearXNG instance's JSON API, via `base_url`).
pub async fn web_search(query: &str, config: &WebSearchConfig) -> ToolResult<Value> {
    let max = if config.max_results == 0 {
        5
    } else {
        config.max_results as usize
    };
    match config.backend.as_str() {
        "duckduckgo" => duckduckgo(query, max).await,
        "searxng" => searxng(query, max, config).await,
        other => Err(ToolError::new(format!(
            "search backend {other:?} not supported (use 'duckduckgo' or 'searxng')"
        ))),
    }
}

fn client(timeout_secs: u64) -> ToolResult<reqwest::Client> {
    crate::http::http_client(timeout_secs, "Mozilla/5.0 flows/0.2", true)
}

async fn duckduckgo(query: &str, max: usize) -> ToolResult<Value> {
    let resp = client(10)?
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| ToolError::new(format!("search request failed: {e}")))?;
    let html = resp
        .text()
        .await
        .map_err(|e| ToolError::new(format!("search read failed: {e}")))?;
    Ok(Value::Array(parse_ddg_html(&html, max)))
}

/// SearXNG JSON API: `GET {base_url}/search?q=..&format=json`. Requires an
/// instance with the JSON output enabled (self-hosted or a private one) — public
/// instances almost always disable it. Maps `results[].{title,url,content}` to
/// our `{title,url,snippet}`.
async fn searxng(query: &str, max: usize, config: &WebSearchConfig) -> ToolResult<Value> {
    let base = config.base_url.trim_end_matches('/');
    if base.is_empty() {
        return Err(ToolError::new(
            "searxng backend requires tools.web_search.base_url (e.g. http://host:8080)"
                .to_string(),
        ));
    }
    let url = format!("{base}/search");
    let mut req = client(12)?
        .get(&url)
        .query(&[("q", query), ("format", "json")]);
    if !config.api_key.is_empty() {
        // Some instances gate the JSON API behind a token/basic auth header.
        req = req.header("Authorization", format!("Bearer {}", config.api_key));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ToolError::new(format!("searxng request failed: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ToolError::new(format!("searxng read failed: {e}")))?;
    if !status.is_success() {
        let head: String = text.chars().take(200).collect();
        return Err(ToolError::new(format!(
            "searxng HTTP {status} (JSON API enabled on the instance?): {head}"
        )));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| {
        ToolError::new(format!(
            "searxng returned non-JSON (is `format: json` enabled?): {e}"
        ))
    })?;
    Ok(Value::Array(parse_searxng(&v, max)))
}

/// Map a SearXNG `{results: [{title,url,content}]}` body to our result shape.
pub fn parse_searxng(v: &Value, max: usize) -> Vec<Value> {
    v.get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(max)
        .map(|r| {
            json!({
                "title": r.get("title").and_then(Value::as_str).unwrap_or(""),
                "url": r.get("url").and_then(Value::as_str).unwrap_or(""),
                "snippet": r.get("content").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect()
}
