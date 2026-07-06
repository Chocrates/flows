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

        let full = resp
            .text()
            .await
            .map_err(|e| ToolError::new(format!("Request failed: {e}")))?;
        let truncated = full.len() > MAX_BYTES;
        let body: String = full.chars().take(MAX_BYTES).collect();
        return Ok(serde_json::json!({
            "status": status.as_u16(),
            "body": body,
            "truncated": truncated,
        }));
    }
    Err(ToolError::new("too many redirects"))
}
