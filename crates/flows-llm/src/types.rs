//! Provider-neutral message + completion types. The agent loop speaks these;
//! each provider module translates to/from its wire format.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool call requested by the model. `arguments` is a JSON string (matching
/// the OpenAI convention the agent loop uses).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// For `Tool` messages: which tool call this result answers.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(text.into()),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
    pub fn assistant_tools(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
        }
    }
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: vec![],
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// A tool the model may call. `parameters` is a JSON Schema object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// The parsed result of one model round.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub content: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub stop_reason: Option<String>,
}

impl Completion {
    /// Assemble a `Completion` from a stream accumulator's parts: normalize empty
    /// tool-call arguments to `{}` and fold reasoning into an `Option`. Shared by
    /// both providers' `finish()` — only their stop-reason field name differs.
    pub(crate) fn from_stream_parts(
        content: String,
        reasoning: String,
        mut tool_calls: Vec<ToolCall>,
        input_tokens: u64,
        output_tokens: u64,
        stop_reason: Option<String>,
    ) -> Self {
        for tc in &mut tool_calls {
            if tc.arguments.is_empty() {
                tc.arguments = "{}".to_string();
            }
        }
        Self {
            content,
            reasoning: (!reasoning.is_empty()).then_some(reasoning),
            tool_calls,
            usage: Usage {
                input_tokens,
                output_tokens,
            },
            stop_reason,
        }
    }
}

/// Per-request options shared across providers.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
    /// Extended-thinking budget in tokens, if enabled.
    pub thinking_budget: Option<u32>,
    /// Keep `tools` declared (history may hold tool calls) but forbid calling
    /// them this turn, so the model must answer in text.
    pub no_tool_calls: bool,
    /// Thinking was explicitly turned off for this phase. Anthropic needs nothing
    /// (no budget = no thinking); local OpenAI-compatible servers get it as a
    /// chat-template switch.
    pub disable_thinking: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("http error: {0}")]
    Http(String),
    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("failed to parse response: {0}")]
    Parse(String),
}
