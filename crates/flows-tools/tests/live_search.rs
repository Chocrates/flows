// Live network test — run explicitly: cargo test -p flows-tools --test live_search -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn live_ddg_search() {
    let cfg = flows_core::models::WebSearchConfig {
        backend: "duckduckgo".into(),
        max_results: 3,
        api_key: String::new(),
        base_url: String::new(),
    };
    let res = flows_tools::search::web_search("rust programming language", &cfg)
        .await
        .expect("search failed");
    let arr = res.as_array().unwrap();
    println!("got {} results", arr.len());
    for r in arr {
        println!(
            "- {}\n  {}",
            r["title"].as_str().unwrap_or("?"),
            r["url"].as_str().unwrap_or("?")
        );
    }
    assert!(!arr.is_empty(), "no results parsed");
    assert!(arr[0]["url"].as_str().unwrap().starts_with("http"));
}
