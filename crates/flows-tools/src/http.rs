//! Port of `flows/tools/http.py` — GET-only fetch with private-IP blocking.
//!
//! Fixes the redirect-revalidation gap flagged in the Python audit: redirects
//! are followed manually and EACH hop's host is re-checked against the private-IP
//! blocklist, so a public URL cannot 3xx-redirect to `169.254.169.254` (cloud
//! metadata) or any internal address.

use std::net::{IpAddr, ToSocketAddrs};

use reqwest::Url;

use crate::error::{ToolError, ToolResult};

const MAX_BYTES: usize = 100_000;
const TIMEOUT_SECS: u64 = 10;
const MAX_REDIRECTS: u32 = 5;

/// Heuristic: a body that opens with a doctype/`<html` is HTML even if the server
/// mislabels the content-type.
fn looks_like_html(s: &str) -> bool {
    let head = s.trim_start().get(..512).unwrap_or(s).to_ascii_lowercase();
    head.contains("<!doctype html") || head.contains("<html")
}

/// Remove every `<tag>…</tag>` block (case-insensitive) including its contents —
/// used to drop `<script>`/`<style>`, whose bodies would otherwise survive a
/// naive tag strip.
fn drop_element(mut s: String, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    loop {
        let lower = s.to_ascii_lowercase();
        let Some(start) = lower.find(&open) else { break };
        // Skip to the end of the opening tag, then to the matching close tag.
        let Some(open_end) = lower[start..].find('>').map(|g| start + g + 1) else {
            break;
        };
        let end = lower[open_end..]
            .find(&close)
            .map(|e| open_end + e + close.len())
            .unwrap_or(s.len());
        s.replace_range(start..end, " ");
    }
    s
}

/// Convert an HTML document to readable text: drop script/style blocks, strip the
/// remaining tags, decode common entities, and collapse runs of whitespace (a
/// newline is preserved when the run contained one). Keeps doc pages from dumping
/// markup + inline JS into the agent's context.
fn html_to_text(html: &str) -> String {
    let mut s = drop_element(html.to_string(), "script");
    s = drop_element(s, "style");

    // Strip tags.
    let mut text = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    // Collapse whitespace: each run → one space, or one newline if it spanned one.
    let mut out = String::with_capacity(text.len());
    let (mut in_ws, mut had_nl) = (false, false);
    for c in text.chars() {
        if c.is_whitespace() {
            in_ws = true;
            had_nl |= c == '\n';
        } else {
            if in_ws && !out.is_empty() {
                out.push(if had_nl { '\n' } else { ' ' });
            }
            in_ws = false;
            had_nl = false;
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// True if an IP is private/loopback/link-local/unique-local — must never be
/// reachable via this tool (SSRF guard).
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                // CGNAT / carrier shared space 100.64.0.0/10
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 0x40)
        }
        IpAddr::V6(v6) => {
            // An IPv4-mapped address (::ffff:a.b.c.d) routes to the embedded v4
            // host, so decode + apply the v4 rules — else ::ffff:127.0.0.1 would
            // slip past as "just a v6 address" and reach loopback.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                // unique local fc00::/7
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // link-local fe80::/10
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Validate a URL's scheme + host, then resolve and block private targets.
pub fn check_url(url: &str) -> ToolResult<Url> {
    let parsed = Url::parse(url).map_err(|e| ToolError::new(format!("invalid url: {e}")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ToolError::new(format!(
                "Only http/https allowed, got: {other:?}"
            )))
        }
    }
    let host = parsed.host_str().unwrap_or("");
    if host.is_empty() || host == "localhost" {
        return Err(ToolError::new(format!("Private host blocked: {host:?}")));
    }
    // Resolve and block private IPs.
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|_| ToolError::new(format!("Could not resolve host: {host:?}")))?;
    let mut any = false;
    for addr in addrs {
        any = true;
        if is_blocked_ip(addr.ip()) {
            return Err(ToolError::new(format!("Private IP blocked: {}", addr.ip())));
        }
    }
    if !any {
        return Err(ToolError::new(format!("Could not resolve host: {host:?}")));
    }
    Ok(parsed)
}

/// GET a URL (following redirects, re-validating each hop). Returns
/// `{status, body, truncated}`.
pub async fn http_get(url: &str) -> ToolResult<serde_json::Value> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .user_agent("flows/0.2")
        .build()
        .map_err(|e| ToolError::new(format!("client build failed: {e}")))?;

    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let checked = check_url(&current)?; // re-validate EVERY hop
        let resp = client
            .get(checked)
            .send()
            .await
            .map_err(|e| ToolError::new(format!("Request failed: {e}")))?;
        let status = resp.status();

        if status.is_redirection() {
            match resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            {
                Some(loc) => {
                    // Resolve relative redirects against the current URL.
                    current = Url::parse(&current)
                        .and_then(|base| base.join(loc))
                        .map(|u| u.to_string())
                        .unwrap_or_else(|_| loc.to_string());
                    continue;
                }
                None => return Err(ToolError::new("redirect without Location header")),
            }
        }

        let is_html = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("html"))
            .unwrap_or(false);
        let full = resp
            .text()
            .await
            .map_err(|e| ToolError::new(format!("Request failed: {e}")))?;
        // HTML pages (e.g. docs.rs) are mostly markup + inline JS/CSS — returning
        // them raw dumps tens of KB into the agent's context. Convert to readable
        // text (drop <script>/<style> blocks and tags) so only the prose/signatures
        // remain. Non-HTML (JSON APIs, plain text) is passed through untouched.
        let (rendered, converted) = if is_html || looks_like_html(&full) {
            (html_to_text(&full), true)
        } else {
            (full, false)
        };
        let truncated = rendered.len() > MAX_BYTES;
        let body: String = rendered.chars().take(MAX_BYTES).collect();
        return Ok(serde_json::json!({
            "status": status.as_u16(),
            "body": body,
            "truncated": truncated,
            "html_converted": converted,
        }));
    }
    Err(ToolError::new("too many redirects"))
}

#[cfg(test)]
mod html_tests {
    use super::*;

    #[test]
    fn strips_script_style_and_tags() {
        let html = "<html><head><style>.x{color:red}</style>\
            <script>var secret = 'leak';</script></head>\
            <body><h1>Bevy</h1><p>App &amp; ECS</p></body></html>";
        let text = html_to_text(html);
        assert!(!text.contains("secret"), "script body leaked: {text}");
        assert!(!text.contains("color:red"), "style body leaked: {text}");
        assert!(!text.contains('<'), "tags leaked: {text}");
        assert!(text.contains("Bevy"), "{text}");
        assert!(text.contains("App & ECS"), "entity not decoded: {text}");
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(html_to_text("<p>a</p>\n\n\n   <p>b</p>"), "a\nb");
    }

    #[test]
    fn looks_like_html_detects_doctype() {
        assert!(looks_like_html("  <!DOCTYPE html><html>"));
        assert!(looks_like_html("<HTML>"));
        assert!(!looks_like_html(r#"{"json": true}"#));
    }
}
