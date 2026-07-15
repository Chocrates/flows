//! OpenAI-compatible embeddings client (`POST {api_base}/embeddings`). Used by
//! the runner's semantic retrieval. Talks to the same trusted, user-configured
//! endpoint as the chat provider (a local llama.cpp / vLLM server or OpenAI) —
//! so, like the chat client, it does NOT go through the agent's SSRF `http_get`
//! guard (that guard is for model-supplied URLs; this endpoint is config-supplied).

use crate::types::LlmError;
use serde_json::{json, Value};

/// Embed a batch of inputs, returning one vector per input (order preserved).
/// `api_base` matches the LLM config (e.g. `http://host:8000/v1`); the request
/// hits `{api_base}/embeddings`.
pub async fn embed(
    api_key: &str,
    api_base: &str,
    model: &str,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>, LlmError> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let base = api_base.trim_end_matches('/');
    let url = format!("{base}/embeddings");
    let body = json!({ "model": model, "input": inputs });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;
    if !status.is_success() {
        return Err(LlmError::Api {
            status: status.as_u16(),
            body: text,
        });
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| LlmError::Parse(e.to_string()))?;
    parse_embeddings(&v, inputs.len())
}

/// Extract `data[].embedding` as f32 vectors, ordered by the response's `index`
/// field when present (OpenAI guarantees `data` is index-ordered, but we sort
/// defensively). Errors if the count doesn't match the request.
fn parse_embeddings(v: &Value, expected: usize) -> Result<Vec<Vec<f32>>, LlmError> {
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| LlmError::Parse("embeddings response missing `data` array".into()))?;
    let mut indexed: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());
    for (i, item) in data.iter().enumerate() {
        let idx = item
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(i as u64) as usize;
        let emb = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| LlmError::Parse("embeddings item missing `embedding`".into()))?
            .iter()
            .map(|n| n.as_f64().unwrap_or(0.0) as f32)
            .collect();
        indexed.push((idx, emb));
    }
    if indexed.len() != expected {
        return Err(LlmError::Parse(format!(
            "embeddings count {} != requested {expected}",
            indexed.len()
        )));
    }
    indexed.sort_by_key(|(i, _)| *i);
    Ok(indexed.into_iter().map(|(_, e)| e).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_orders_by_index() {
        let v = json!({
            "data": [
                {"index": 1, "embedding": [0.0, 1.0]},
                {"index": 0, "embedding": [1.0, 0.0]}
            ]
        });
        let out = parse_embeddings(&v, 2).unwrap();
        assert_eq!(out, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[test]
    fn count_mismatch_errors() {
        let v = json!({"data": [{"index": 0, "embedding": [1.0]}]});
        assert!(parse_embeddings(&v, 3).is_err());
    }
}
