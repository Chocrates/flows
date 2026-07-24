//! Port of `flows/tools/http.py` — GET-only fetch with private-IP blocking.
//!
//! Fixes the redirect-revalidation gap flagged in the Python audit: redirects
//! are followed manually and EACH hop's host is re-checked against the private-IP
//! blocklist, so a public URL cannot 3xx-redirect to `169.254.169.254` (cloud
//! metadata) or any internal address.

use std::net::{IpAddr, ToSocketAddrs};

use reqwest::Url;

use crate::error::{ToolError, ToolResult};
use crate::html::{html_to_text, looks_like_html};

const MAX_BYTES: usize = 100_000;
const TIMEOUT_SECS: u64 = 10;
const MAX_REDIRECTS: u32 = 5;

/// Build a reqwest client with the shared knobs (timeout, user-agent, redirect
/// policy). One place so `search`/`lookup`/`http_get` don't each re-spell the
/// builder. `follow_redirects: false` uses `Policy::none()` (the caller handles
/// redirects itself, as `http_get` does for SSRF re-validation).
pub fn http_client(
    timeout_secs: u64,
    user_agent: &str,
    follow_redirects: bool,
) -> ToolResult<reqwest::Client> {
    let redirect = if follow_redirects {
        reqwest::redirect::Policy::default()
    } else {
        reqwest::redirect::Policy::none()
    };
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent(user_agent)
        .redirect(redirect)
        .build()
        .map_err(|e| ToolError::new(format!("client build failed: {e}")))
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
    // Redirects handled manually below so each hop is re-validated against the
    // SSRF blocklist — so `follow_redirects: false`.
    let client = http_client(TIMEOUT_SECS, "flows/0.2", false)?;

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
