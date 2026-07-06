//! OpenAI Chat Completions translation (pure — no network). Covers OpenAI and
//! any OpenAI-compatible endpoint (local servers, gateways). The neutral message
//! types are already OpenAI-shaped, so this is close to 1:1.

use serde_json::{json, Map, Value};

use crate::types::{Completion, CompletionRequest, LlmError, Role, ToolCall, Usage};

fn is_reasoning_model(model: &str) -> bool {
    let m = model.rsplit('/').next().unwrap_or(model);
    m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

/// Build the Chat Completions request body.
pub fn build_body(req: &CompletionRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if !req.system.is_empty() {
        messages.push(json!({"role": "system", "content": req.system}));
    }
    for m in &req.messages {
        match m.role {
            Role::System => messages.push(json!({"role": "system", "content": m.content})),
            Role::User => messages.push(json!({"role": "user", "content": m.content})),
            Role::Assistant => {
                let mut msg = Map::new();
                msg.insert("role".into(), json!("assistant"));
                // content=null (not "") for a tool-only turn.
                msg.insert(
                    "content".into(),
                    m.content.clone().map(Value::String).unwrap_or(Value::Null),
                );
                if !m.tool_calls.is_empty() {
                    let calls: Vec<Value> = m
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {"name": tc.name, "arguments": tc.arguments},
                            })
                        })
                        .collect();
                    msg.insert("tool_calls".into(), json!(calls));
                }
                messages.push(Value::Object(msg));
            }
            Role::Tool => messages.push(json!({
                "role": "tool",
                "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                "content": m.content,
            })),
        }
    }

    let mut body = Map::new();
    body.insert("model".into(), json!(req.model));
    body.insert("messages".into(), json!(messages));
    // o-series use max_completion_tokens and reject temperature/max_tokens.
    if is_reasoning_model(&req.model) {
        body.insert("max_completion_tokens".into(), json!(req.max_tokens));
    } else {
        body.insert("max_tokens".into(), json!(req.max_tokens));
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({
                "type": "function",
                "function": {"name": t.name, "description": t.description, "parameters": t.parameters},
            }))
            .collect();
        body.insert("tools".into(), json!(tools));
    }
    Value::Object(body)
}

/// Streaming accumulator for Chat Completions SSE chunks (OpenAI + llama.cpp).
/// Processes one decoded chunk at a time, emitting text deltas via `on_token`.
#[derive(Default)]
pub struct StreamAccumulator {
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    input_tokens: u64,
    output_tokens: u64,
    finish_reason: Option<String>,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process(
        &mut self,
        chunk: &Value,
        on_token: &(dyn Fn(&str) + Send + Sync),
        on_thinking: &(dyn Fn(&str) + Send + Sync),
    ) {
        if let Some(u) = chunk.get("usage") {
            if let Some(p) = u.get("prompt_tokens").and_then(Value::as_u64) {
                self.input_tokens = p;
            }
            if let Some(c) = u.get("completion_tokens").and_then(Value::as_u64) {
                self.output_tokens = c;
            }
        }
        let Some(choice) = chunk.get("choices").and_then(|c| c.get(0)) else {
            return;
        };
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(fr.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return;
        };

        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            self.content.push_str(text);
            on_token(text);
        }
        // Some OpenAI-compatible servers (deepseek, llama.cpp with reasoning
        // models) stream reasoning in a separate `reasoning_content` delta.
        if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
            self.reasoning.push_str(r);
            on_thinking(r);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in calls {
                let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                while self.tool_calls.len() <= idx {
                    self.tool_calls.push(ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    });
                }
                let slot = &mut self.tool_calls[idx];
                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    if !id.is_empty() {
                        slot.id = id.to_string();
                    }
                }
                if let Some(f) = tc.get("function") {
                    if let Some(name) = f.get("name").and_then(Value::as_str) {
                        if !name.is_empty() {
                            slot.name = name.to_string();
                        }
                    }
                    if let Some(args) = f.get("arguments").and_then(Value::as_str) {
                        slot.arguments.push_str(args);
                    }
                }
            }
        }
    }

    pub fn finish(mut self) -> Completion {
        for tc in &mut self.tool_calls {
            if tc.arguments.is_empty() {
                tc.arguments = "{}".to_string();
            }
        }
        Completion {
            content: self.content,
            reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
            tool_calls: self.tool_calls,
            usage: Usage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
            },
            stop_reason: self.finish_reason,
        }
    }
}

/// Parse a Chat Completions response into a neutral `Completion`.
pub fn parse_response(resp: &Value) -> Result<Completion, LlmError> {
    let message = resp
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .ok_or_else(|| LlmError::Parse("missing choices[0].message".into()))?;

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|tc| {
                    let f = tc.get("function");
                    ToolCall {
                        id: tc
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        name: f
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        arguments: f
                            .and_then(|f| f.get("arguments"))
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let usage = resp
        .get("usage")
        .map(|u| Usage {
            input_tokens: u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
            output_tokens: u
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        })
        .unwrap_or_default();

    let reasoning = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Ok(Completion {
        content,
        reasoning,
        tool_calls,
        usage,
        stop_reason: resp
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("finish_reason"))
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}
