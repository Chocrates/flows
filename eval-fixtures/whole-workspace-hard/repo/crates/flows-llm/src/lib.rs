//! flows-llm — provider-neutral LLM client: a `Provider` trait with Anthropic
//! Messages and OpenAI Chat Completions backends (translation + streaming SSE,
//! tool calls, extended thinking / reasoning). `make_provider` selects by model.

pub mod anthropic;
pub mod openai;
pub mod provider;
pub mod stream;
pub mod types;

pub use provider::{make_provider, AnthropicProvider, Emit, OpenAiProvider, Provider};
pub use stream::Accumulator;
pub use types::{Completion, CompletionRequest, LlmError, Message, Role, ToolCall, ToolDef, Usage};
