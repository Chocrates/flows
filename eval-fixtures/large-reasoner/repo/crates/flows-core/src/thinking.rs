//! Extended-thinking resolution. Port of `_resolve_thinking` /
//! `_model_supports_thinking` from `flows/phases/agent.py`.
//!
//! `model_supports_thinking` replaces litellm's `supports_reasoning` with a
//! heuristic over known reasoning-capable model families (refined in flows-llm
//! once real provider metadata is available).

use crate::models::{LLMConfig, PhaseConfig};

pub const DEFAULT_THINKING_BUDGET: u32 = 2048;

/// Effective (enabled, budget_tokens). Precedence: phase > llm > default-on.
pub fn resolve_thinking(phase: &PhaseConfig, llm: &LLMConfig) -> (bool, u32) {
    let enabled = phase.thinking.or(llm.thinking).unwrap_or(true);
    let budget = llm
        .thinking_budget_tokens
        .unwrap_or(DEFAULT_THINKING_BUDGET);
    (enabled, budget)
}

/// Whether a model supports extended thinking / reasoning tokens.
pub fn model_supports_thinking(model: &str) -> bool {
    let m = model.to_lowercase();
    // Anthropic extended thinking (claude 3.7+, claude 4.x, fable)
    m.contains("claude-opus-4")
        || m.contains("claude-sonnet-4")
        || m.contains("claude-haiku-4")
        || m.contains("claude-3-7")
        || m.contains("claude-fable")
        // OpenAI reasoning models
        || strip_provider(&m).starts_with("o1")
        || strip_provider(&m).starts_with("o3")
        // Google
        || m.contains("gemini-2.5")
}

fn strip_provider(m: &str) -> &str {
    m.rsplit('/').next().unwrap_or(m)
}
