//! Anthropic streaming (SSE) accumulator — pure, testable. Processes decoded
//! event JSON objects into a `Completion`, emitting text/thinking deltas via
//! callbacks as they arrive. Port of the streaming half of `_accumulate_stream`.

use std::collections::HashMap;

use serde_json::Value;

use crate::types::{Completion, ToolCall};

#[derive(Default)]
pub struct Accumulator {
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    /// content-block index → position in `tool_calls`
    tool_index: HashMap<u64, usize>,
    input_tokens: u64,
    output_tokens: u64,
    stop_reason: Option<String>,
}

use flows_core::json::str_field as as_str;

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one decoded SSE event. `on_token`/`on_thinking` receive text and
    /// reasoning deltas respectively.
    pub fn process(
        &mut self,
        event: &Value,
        on_token: &(dyn Fn(&str) + Send + Sync),
        on_thinking: &(dyn Fn(&str) + Send + Sync),
    ) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(u) = event.get("message").and_then(|m| m.get("usage")) {
                    self.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                }
            }
            Some("content_block_start") => {
                let idx = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(cb) = event.get("content_block") {
                    if cb.get("type").and_then(Value::as_str) == Some("tool_use") {
                        let pos = self.tool_calls.len();
                        self.tool_calls.push(ToolCall {
                            id: as_str(cb, "id"),
                            name: as_str(cb, "name"),
                            arguments: String::new(),
                        });
                        self.tool_index.insert(idx, pos);
                    }
                }
            }
            Some("content_block_delta") => {
                let idx = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(delta) = event.get("delta") {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let t = as_str(delta, "text");
                            self.content.push_str(&t);
                            on_token(&t);
                        }
                        Some("thinking_delta") => {
                            let t = as_str(delta, "thinking");
                            self.reasoning.push_str(&t);
                            on_thinking(&t);
                        }
                        Some("input_json_delta") => {
                            if let Some(&pos) = self.tool_index.get(&idx) {
                                self.tool_calls[pos]
                                    .arguments
                                    .push_str(&as_str(delta, "partial_json"));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("message_delta") => {
                if let Some(sr) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = Some(sr.to_string());
                }
                if let Some(o) = event
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                {
                    self.output_tokens = o;
                }
            }
            _ => {}
        }
    }

    pub fn finish(self) -> Completion {
        Completion::from_stream_parts(
            self.content,
            self.reasoning,
            self.tool_calls,
            self.input_tokens,
            self.output_tokens,
            self.stop_reason,
        )
    }
}
