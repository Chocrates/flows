//! Port of `flows/tools/search.py` — web search. DuckDuckGo HTML endpoint
//! (no API key). The result parser is pure/testable; the fetch is a thin
//! reqwest call.

use serde_json::{json, Value};

use flows_core::models::WebSearchConfig;

use crate::error::{ToolError, ToolResult};

/// Strip HTML tags and decode a few common entities.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

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

/// Run a web search. DuckDuckGo only for now (brave/serper need API keys).
pub async fn web_search(query: &str, config: &WebSearchConfig) -> ToolResult<Value> {
    if config.backend != "duckduckgo" {
        return Err(ToolError::new(format!(
            "search backend {:?} not supported in this build (only 'duckduckgo')",
            config.backend
        )));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Mozilla/5.0 flows/0.2")
        .build()
        .map_err(|e| ToolError::new(format!("client build failed: {e}")))?;
    let resp = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| ToolError::new(format!("search request failed: {e}")))?;
    let html = resp
        .text()
        .await
        .map_err(|e| ToolError::new(format!("search read failed: {e}")))?;
    let max = if config.max_results == 0 {
        5
    } else {
        config.max_results as usize
    };
    Ok(Value::Array(parse_ddg_html(&html, max)))
}
