//! web_search parser tests (pure — fixture HTML, no network).

use flows_core::models::WebSearchConfig;
use flows_tools::search::{
    backend_key_with, parse_brave, parse_ddg_html, parse_searxng, parse_serper,
};
use serde_json::json;

const FIXTURE: &str = r##"
<div class="result">
  <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=x">First &amp; Best Result</a>
  <a class="result__snippet" href="...">A snippet about <b>rust</b> programming.</a>
</div>
<div class="result">
  <a rel="nofollow" class="result__a" href="https://direct.example.org/page">Second Result</a>
  <a class="result__snippet" href="...">Another snippet here.</a>
</div>
<div class="result">
  <a rel="nofollow" class="result__a" href="https://third.example.net/">Third</a>
</div>
"##;

#[test]
fn parses_title_url_snippet() {
    let results = parse_ddg_html(FIXTURE, 10);
    assert_eq!(results.len(), 3);

    assert_eq!(results[0]["title"], "First & Best Result"); // entity + tags stripped
    assert_eq!(results[0]["url"], "https://example.com/a"); // uddg unwrapped + decoded
    assert_eq!(results[0]["snippet"], "A snippet about rust programming.");

    assert_eq!(results[1]["url"], "https://direct.example.org/page"); // direct href
    assert_eq!(results[2]["title"], "Third");
    assert_eq!(results[2]["snippet"], ""); // no snippet block
}

#[test]
fn respects_max_results() {
    let results = parse_ddg_html(FIXTURE, 2);
    assert_eq!(results.len(), 2);
}

#[test]
fn empty_html_yields_nothing() {
    assert!(parse_ddg_html("<html><body>no results</body></html>", 5).is_empty());
}

#[tokio::test]
async fn unknown_backend_errors() {
    let cfg = WebSearchConfig {
        backend: "bing".into(),
        ..Default::default()
    };
    let err = flows_tools::search::web_search("rust", &cfg)
        .await
        .unwrap_err();
    assert!(err.0.contains("not supported"));
    assert!(err.0.contains("brave") && err.0.contains("serper"));
}

#[tokio::test]
async fn searxng_without_base_url_errors() {
    let cfg = WebSearchConfig {
        backend: "searxng".into(),
        ..Default::default()
    };
    let err = flows_tools::search::web_search("rust", &cfg)
        .await
        .unwrap_err();
    assert!(err.0.contains("base_url"), "{}", err.0);
}

#[test]
fn parse_searxng_maps_results() {
    let body = serde_json::json!({
        "results": [
            {"title": "Bevy", "url": "https://bevyengine.org", "content": "A game engine"},
            {"title": "Docs", "url": "https://docs.rs/bevy", "content": "API"}
        ]
    });
    let out = parse_searxng(&body, 5);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["title"], "Bevy");
    assert_eq!(out[0]["url"], "https://bevyengine.org");
    assert_eq!(out[0]["snippet"], "A game engine"); // content → snippet
                                                    // respects max
    assert_eq!(parse_searxng(&body, 1).len(), 1);
}

// -- keyed backends (Brave, Serper) ------------------------------------------

#[test]
fn parse_brave_maps_and_strips_highlights() {
    let body = json!({"web": {"results": [
        {"title": "The <strong>Rust</strong> Book", "url": "https://doc.rust-lang.org/book/",
         "description": "Learn <strong>Rust</strong> &amp; more"},
        {"title": "b", "url": "https://b", "description": ""}
    ]}});
    let out = parse_brave(&body, 5);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["title"], "The Rust Book");
    assert_eq!(out[0]["url"], "https://doc.rust-lang.org/book/");
    assert!(out[0]["snippet"]
        .as_str()
        .unwrap()
        .starts_with("Learn Rust"));
    assert_eq!(parse_brave(&body, 1).len(), 1);
    assert!(parse_brave(&json!({}), 5).is_empty());
}

#[test]
fn parse_serper_maps_organic() {
    let body = json!({"organic": [
        {"title": "Tokio", "link": "https://tokio.rs", "snippet": "async runtime"}
    ], "knowledgeGraph": {}});
    let out = parse_serper(&body, 5);
    assert_eq!(
        out,
        vec![json!({"title": "Tokio", "url": "https://tokio.rs", "snippet": "async runtime"})]
    );
    assert!(parse_serper(&json!({"organic": []}), 5).is_empty());
}

#[test]
fn backend_key_prefers_config_then_env() {
    let cfg = |backend: &str, key: &str| WebSearchConfig {
        backend: backend.into(),
        api_key: key.into(),
        ..Default::default()
    };
    let env = |k: &str| match k {
        "BRAVE_API_KEY" => Some("env-brave".to_string()),
        "SERPER_API_KEY" => Some(String::new()),
        _ => None,
    };
    assert_eq!(
        backend_key_with(&cfg("brave", "cfg"), env).as_deref(),
        Some("cfg")
    );
    assert_eq!(
        backend_key_with(&cfg("brave", ""), env).as_deref(),
        Some("env-brave")
    );
    assert_eq!(backend_key_with(&cfg("serper", ""), env), None); // empty env ignored
    assert_eq!(backend_key_with(&cfg("duckduckgo", "x"), env), None);
}

/// One-shot HTTP server on 127.0.0.1: returns `(base_url, handle)`; the handle
/// yields the raw request it received after replying `status` + `body`.
fn mock_server(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/search", listener.local_addr().unwrap());
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let h = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = sock.read(&mut chunk).unwrap();
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if buf.len() >= end + 4 + len || n == 0 {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        sock.write_all(reply.as_bytes()).unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    });
    (url, h)
}

#[tokio::test]
async fn brave_sends_token_and_count() {
    let (url, h) = mock_server(
        "200 OK",
        r#"{"web":{"results":[{"title":"t","url":"https://u","description":"d"}]}}"#,
    );
    let cfg = WebSearchConfig {
        backend: "brave".into(),
        api_key: "BRAVE-KEY".into(),
        base_url: url,
        max_results: 3,
    };
    let out = flows_tools::search::web_search("rust lang", &cfg)
        .await
        .unwrap();
    assert_eq!(out[0]["url"], "https://u");
    let req = h.join().unwrap();
    let lower = req.to_ascii_lowercase();
    assert!(req.starts_with("GET /search?q=rust+lang&count=3 "), "{req}");
    assert!(lower.contains("x-subscription-token: brave-key"), "{req}");
}

#[tokio::test]
async fn serper_posts_query_with_key() {
    let (url, h) = mock_server(
        "200 OK",
        r#"{"organic":[{"title":"t","link":"https://l","snippet":"s"}]}"#,
    );
    let cfg = WebSearchConfig {
        backend: "serper".into(),
        api_key: "SERPER-KEY".into(),
        base_url: url,
        max_results: 4,
    };
    let out = flows_tools::search::web_search("tokio", &cfg)
        .await
        .unwrap();
    assert_eq!(out[0]["url"], "https://l");
    let req = h.join().unwrap();
    assert!(req.starts_with("POST /search "), "{req}");
    assert!(
        req.to_ascii_lowercase().contains("x-api-key: serper-key"),
        "{req}"
    );
    assert!(
        req.ends_with(r#"{"num":4,"q":"tokio"}"#) || req.ends_with(r#"{"q":"tokio","num":4}"#),
        "{req}"
    );
}

#[tokio::test]
async fn keyed_backend_auth_error_is_explained() {
    let (url, h) = mock_server("401 Unauthorized", r#"{"message":"bad key"}"#);
    let cfg = WebSearchConfig {
        backend: "serper".into(),
        api_key: "nope".into(),
        base_url: url,
        max_results: 5,
    };
    let err = flows_tools::search::web_search("x", &cfg)
        .await
        .unwrap_err();
    h.join().unwrap();
    assert!(
        err.0.contains("401") && err.0.contains("check the API key"),
        "{}",
        err.0
    );
}
