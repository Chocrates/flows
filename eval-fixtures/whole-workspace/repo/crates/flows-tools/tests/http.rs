//! http tool tests. The SSRF guard (is_blocked_ip / check_url) is the
//! security-critical part and is pure/testable; the live fetch is gated.

use std::net::IpAddr;

use flows_tools::http::{check_url, is_blocked_ip};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn blocks_private_and_loopback_ipv4() {
    assert!(is_blocked_ip(ip("10.0.0.1")));
    assert!(is_blocked_ip(ip("172.16.5.4")));
    assert!(is_blocked_ip(ip("192.168.1.1")));
    assert!(is_blocked_ip(ip("127.0.0.1")));
    assert!(is_blocked_ip(ip("169.254.169.254"))); // cloud metadata
    assert!(is_blocked_ip(ip("0.0.0.0")));
}

#[test]
fn allows_public_ipv4() {
    assert!(!is_blocked_ip(ip("8.8.8.8")));
    assert!(!is_blocked_ip(ip("1.1.1.1")));
}

#[test]
fn blocks_ipv6_loopback_and_ula() {
    assert!(is_blocked_ip(ip("::1")));
    assert!(is_blocked_ip(ip("fc00::1")));
    assert!(is_blocked_ip(ip("fe80::1")));
    assert!(!is_blocked_ip(ip("2606:4700:4700::1111"))); // public
}

#[test]
fn blocks_ipv4_mapped_ipv6_loopback() {
    // ::ffff:127.0.0.1 routes to 127.0.0.1 — must be blocked
    assert!(is_blocked_ip(ip("::ffff:127.0.0.1")));
    assert!(is_blocked_ip(ip("::ffff:169.254.169.254"))); // mapped metadata IP
    assert!(is_blocked_ip(ip("::ffff:10.0.0.1")));
    assert!(!is_blocked_ip(ip("::ffff:8.8.8.8"))); // mapped public still allowed
}

#[test]
fn blocks_cgnat_and_broadcast() {
    assert!(is_blocked_ip(ip("100.64.0.1"))); // CGNAT 100.64/10
    assert!(is_blocked_ip(ip("100.127.255.255")));
    assert!(!is_blocked_ip(ip("100.63.0.1"))); // just outside the range
    assert!(!is_blocked_ip(ip("100.128.0.1")));
    assert!(is_blocked_ip(ip("255.255.255.255"))); // broadcast
}

#[test]
fn check_url_rejects_non_http_scheme() {
    let err = check_url("file:///etc/passwd").unwrap_err();
    assert!(err.0.contains("http/https"));
}

#[test]
fn check_url_rejects_localhost() {
    let err = check_url("http://localhost/x").unwrap_err();
    assert!(err.0.contains("Private host"));
}

#[test]
fn check_url_blocks_literal_private_ip() {
    // resolves to the private IP literally → blocked
    let err = check_url("http://169.254.169.254/latest/meta-data").unwrap_err();
    assert!(err.0.contains("Private IP blocked"));
}

#[test]
fn check_url_rejects_garbage() {
    assert!(check_url("not a url").is_err());
}
