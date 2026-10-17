//! Endpoint model listing (`GET /models`), used by `flows doctor` to check that a
//! local OpenAI-compatible server is up and serves the configured model.

use std::time::Duration;

/// List the model ids an OpenAI-compatible endpoint serves. Errors are short,
/// user-facing reasons (timeout, refused, HTTP status, bad body).
pub async fn list_models(
    api_base: &str,
    api_key: &str,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let url = format!("{}/models", api_base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(&url);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() {
            format!(
                "no answer within {}s (down, or busy with another request)",
                timeout.as_secs()
            )
        } else if e.is_connect() {
            "unreachable (connection failed)".to_string()
        } else {
            format!("request failed: {e}")
        }
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| {
        if e.is_timeout() {
            format!("no answer within {}s", timeout.as_secs())
        } else {
            "unexpected response (not a model list)".to_string()
        }
    })?;
    let data = v["data"]
        .as_array()
        .ok_or("unexpected response (no `data` list)")?;
    Ok(data
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect())
}
