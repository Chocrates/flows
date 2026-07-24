//! Provider trait + the Anthropic HTTP client (the only network piece).

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;

use flows_core::models::LLMConfig;

use crate::anthropic;
use crate::retry::{send_with_retry, RetryConfig};
use crate::stream::Accumulator;
use crate::types::{Completion, CompletionRequest, LlmError};

/// Callback for streamed text/reasoning deltas.
pub type Emit<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// An SSE event accumulator: fold decoded `data:` JSON events into a `Completion`,
/// emitting text/reasoning deltas as they arrive. Implemented by each provider's
/// accumulator so the byte-stream framing lives in one place ([`drive_sse`]).
pub trait SseAccumulator {
    fn process(&mut self, event: &Value, on_token: Emit<'_>, on_thinking: Emit<'_>);
    fn finish(self) -> Completion;
}

impl SseAccumulator for Accumulator {
    fn process(&mut self, event: &Value, on_token: Emit<'_>, on_thinking: Emit<'_>) {
        Accumulator::process(self, event, on_token, on_thinking);
    }
    fn finish(self) -> Completion {
        Accumulator::finish(self)
    }
}

impl SseAccumulator for crate::openai::StreamAccumulator {
    fn process(&mut self, event: &Value, on_token: Emit<'_>, on_thinking: Emit<'_>) {
        crate::openai::StreamAccumulator::process(self, event, on_token, on_thinking);
    }
    fn finish(self) -> Completion {
        crate::openai::StreamAccumulator::finish(self)
    }
}

/// Read an SSE byte stream, split it into events on blank lines, and feed each
/// event's `data:` JSON into `acc` (skipping empties and `[DONE]`). Shared by
/// every streaming provider — the framing is identical, only the accumulator and
/// the request differ.
async fn drive_sse<A: SseAccumulator>(
    resp: reqwest::Response,
    mut acc: A,
    on_token: Emit<'_>,
    on_thinking: Emit<'_>,
) -> Result<Completion, LlmError> {
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::Http(e.to_string()))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find("\n\n") {
            let block: String = buf.drain(..pos + 2).collect();
            for line in block.lines() {
                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    if let Ok(ev) = serde_json::from_str::<Value>(data) {
                        acc.process(&ev, on_token, on_thinking);
                    }
                }
            }
        }
    }
    Ok(acc.finish())
}

#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, LlmError>;

    /// Streaming variant. Default falls back to `complete` (no token deltas), so
    /// non-streaming providers and mocks work unchanged.
    async fn complete_streaming(
        &self,
        req: &CompletionRequest,
        _on_token: Emit<'_>,
        _on_thinking: Emit<'_>,
    ) -> Result<Completion, LlmError> {
        self.complete(req).await
    }
}

pub struct AnthropicProvider {
    api_key: String,
    api_base: String,
    client: reqwest::Client,
    retry: RetryConfig,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, api_base: Option<&str>) -> Self {
        let base = api_base
            .filter(|s| !s.is_empty())
            .unwrap_or("https://api.anthropic.com")
            .trim_end_matches('/')
            .to_string();
        Self {
            api_key: api_key.into(),
            api_base: base,
            client: reqwest::Client::new(),
            retry: RetryConfig::from_env(),
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, LlmError> {
        let body = anthropic::build_body(req);
        let url = format!("{}/v1/messages", self.api_base);
        let resp = send_with_retry(&self.retry, || {
            self.client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
        })
        .await?;

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
        anthropic::parse_response(&v)
    }

    async fn complete_streaming(
        &self,
        req: &CompletionRequest,
        on_token: Emit<'_>,
        on_thinking: Emit<'_>,
    ) -> Result<Completion, LlmError> {
        let mut body = anthropic::build_body(req);
        body.as_object_mut()
            .unwrap()
            .insert("stream".into(), Value::Bool(true));
        let url = format!("{}/v1/messages", self.api_base);

        let resp = send_with_retry(&self.retry, || {
            self.client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
        })
        .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api { status, body });
        }

        drive_sse(resp, Accumulator::new(), on_token, on_thinking).await
    }
}

/// OpenAI-compatible Chat Completions client (OpenAI, local servers, gateways).
pub struct OpenAiProvider {
    api_key: String,
    api_base: String,
    client: reqwest::Client,
    retry: RetryConfig,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>, api_base: Option<&str>) -> Self {
        let base = api_base
            .filter(|s| !s.is_empty())
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/')
            .to_string();
        Self {
            api_key: api_key.into(),
            api_base: base,
            client: reqwest::Client::new(),
            retry: RetryConfig::from_env(),
        }
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, LlmError> {
        let body = crate::openai::build_body(req);
        let url = format!("{}/chat/completions", self.api_base);
        let resp = send_with_retry(&self.retry, || {
            self.client
                .post(&url)
                .bearer_auth(&self.api_key)
                .header("content-type", "application/json")
                .json(&body)
        })
        .await?;
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
        crate::openai::parse_response(&v)
    }

    async fn complete_streaming(
        &self,
        req: &CompletionRequest,
        on_token: Emit<'_>,
        on_thinking: Emit<'_>,
    ) -> Result<Completion, LlmError> {
        let mut body = crate::openai::build_body(req);
        let obj = body.as_object_mut().unwrap();
        obj.insert("stream".into(), Value::Bool(true));
        // Ask for usage in the final chunk (OpenAI honors; llama.cpp/others ignore).
        obj.insert(
            "stream_options".into(),
            serde_json::json!({"include_usage": true}),
        );
        let url = format!("{}/chat/completions", self.api_base);

        let resp = send_with_retry(&self.retry, || {
            self.client
                .post(&url)
                .bearer_auth(&self.api_key)
                .header("content-type", "application/json")
                .json(&body)
        })
        .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api { status, body });
        }

        drive_sse(
            resp,
            crate::openai::StreamAccumulator::new(),
            on_token,
            on_thinking,
        )
        .await
    }
}

fn is_anthropic_model(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("claude") || m.starts_with("anthropic/")
}

/// Pick a provider for the configured model: Anthropic for claude, otherwise an
/// OpenAI-compatible endpoint (respecting `api_base` for local/gateway servers).
pub fn make_provider(llm: &LLMConfig) -> Box<dyn Provider> {
    let api_base = if llm.api_base.is_empty() {
        None
    } else {
        Some(llm.api_base.as_str())
    };
    if is_anthropic_model(&llm.model) {
        Box::new(AnthropicProvider::new(llm.api_key.clone(), api_base))
    } else {
        Box::new(OpenAiProvider::new(llm.api_key.clone(), api_base))
    }
}
