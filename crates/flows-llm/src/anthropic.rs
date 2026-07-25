//! Anthropic Messages API translation (pure — no network).
//!
//! Neutral messages → Anthropic content blocks and back. Replaces litellm's
//! provider translation for claude models. The native format sidesteps the
//! empty-content placeholder bug entirely: a tool-only assistant turn simply
//! emits `tool_use` blocks with no text block.

use serde_json::{json, Map, Value};

use crate::types::{Completion, CompletionRequest, LlmError, Message, Role, ToolCall, Usage};

/// Build the Anthropic Messages API request body.
pub fn build_body(req: &CompletionRequest) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("model".into(), json!(req.model));

    // Anthropic requires max_tokens; with thinking it must exceed the budget.
    let max_tokens = match req.thinking_budget {
        Some(budget) => req.max_tokens.max(budget + 4096),
        None => req.max_tokens,
    };
    body.insert("max_tokens".into(), json!(max_tokens));

    if !req.system.is_empty() {
        body.insert("system".into(), json!(req.system));
    }
    body.insert("messages".into(), json!(build_messages(&req.messages)));

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
            .collect();
        body.insert("tools".into(), json!(tools));
    }

    if let Some(budget) = req.thinking_budget {
        body.insert(
            "thinking".into(),
            json!({"type": "enabled", "budget_tokens": budget}),
        );
    }

    body
}

/// Translate neutral messages into Anthropic message objects, merging
/// consecutive tool results into a single user turn (as Anthropic expects).
fn build_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();

    for m in messages {
        match m.role {
            // System is a top-level field; ignore stray system messages here.
            Role::System => {}
            Role::User => {
                out.push(json!({
                    "role": "user",
                    "content": [{"type": "text", "text": m.content.clone().unwrap_or_default()}],
                }));
            }
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(text) = &m.content {
                    if !text.is_empty() {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                for tc in &m.tool_calls {
                    let input: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                    blocks.push(
                        json!({"type": "tool_use", "id": tc.id, "name": tc.name, "input": input}),
                    );
                }
                out.push(json!({"role": "assistant", "content": blocks}));
            }
            Role::Tool => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content.clone().unwrap_or_default(),
                });
                // Merge into the previous user turn if it's already tool results.
                if let Some(last) = out.last_mut() {
                    if last.get("role").and_then(Value::as_str) == Some("user")
                        && is_tool_result_turn(last)
                    {
                        last["content"].as_array_mut().unwrap().push(block);
                        continue;
                    }
                }
                out.push(json!({"role": "user", "content": [block]}));
            }
        }
    }

    out
}

fn is_tool_result_turn(msg: &Value) -> bool {
    msg.get("content")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .all(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        })
        .unwrap_or(false)
}

/// Parse an Anthropic Messages API response into a neutral `Completion`.
pub fn parse_response(resp: &Value) -> Result<Completion, LlmError> {
    let content_blocks = resp
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| LlmError::Parse("missing 'content' array".into()))?;

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in content_blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                content.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""));
            }
            Some("thinking") => {
                reasoning.push_str(block.get("thinking").and_then(Value::as_str).unwrap_or(""));
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let arguments = block
                    .get("input")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            _ => {}
        }
    }

    let usage = resp
        .get("usage")
        .map(|u| Usage {
            input_tokens: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            output_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
        .unwrap_or_default();

    Ok(Completion {
        content,
        reasoning: (!reasoning.is_empty()).then_some(reasoning),
        tool_calls,
        usage,
        stop_reason: resp
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}
