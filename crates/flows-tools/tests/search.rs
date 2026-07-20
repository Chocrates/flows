//! web_search parser tests (pure — fixture HTML, no network).

use flows_core::models::WebSearchConfig;
use flows_tools::search::{parse_ddg_html, parse_searxng};

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
        backend: "brave".into(),
        ..Default::default()
    };
    let err = flows_tools::search::web_search("rust", &cfg)
        .await
        .unwrap_err();
    assert!(err.0.contains("not supported"));
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
