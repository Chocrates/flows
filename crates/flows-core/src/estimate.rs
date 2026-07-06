//! Port of `flows/estimate.py` — per-phase cost estimate, no LLM calls.
//!
//! Token counting uses a real BPE tokenizer (tiktoken) for OpenAI-family models
//! and a chars/4 heuristic for everything else (Anthropic, local/llama.cpp — the
//! latter cost $0 so precision is moot). Prompts are estimated with a stub
//! (the real prompt render isn't needed for a ballpark).

use std::path::Path;

use crate::models::{LLMConfig, PhaseConfig, PhaseType};
use crate::pricing::lookup_pricing;
use crate::thinking::{model_supports_thinking, resolve_thinking};

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseEstimate {
    pub phase: String,
    pub phase_type: PhaseType,
    pub model: String,
    pub in_tok: u64,
    pub out_tok_mid: u64,
    pub cost_low: f64,
    pub cost_high: f64,
}

const DEFAULT_OUT_TOKENS: u64 = 600;

fn out_heuristic(name: &str) -> u64 {
    match name {
        "plan" => 600,
        "requirements" => 800,
        "spec" => 1200,
        "design" => 1000,
        "implement" => 2000,
        "gather" => 1000,
        "synthesize" => 800,
        "report" => 1500,
        "review" => 500,
        "approve" => 100,
        "run" => 600,
        "analyze" => 800,
        "findings" => 600,
        "recommendations" => 500,
        _ => DEFAULT_OUT_TOKENS,
    }
}

/// Approximate token count. Uses a real BPE tokenizer for OpenAI-family models;
/// everything else (Anthropic, local/llama.cpp, unknown) falls back to chars/4
/// — good enough, and local models cost $0 so precision doesn't matter there.
pub fn count_tokens(model: &str, text: &str) -> u64 {
    use std::sync::OnceLock;
    use tiktoken_rs::CoreBPE;

    let m = model.rsplit('/').next().unwrap_or(model).to_lowercase();
    // o200k for gpt-4o / o-series; cl100k for older gpt-4 / gpt-3.5.
    let bpe: Option<&CoreBPE> = if m.starts_with("gpt-4o")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
    {
        static O200K: OnceLock<Option<CoreBPE>> = OnceLock::new();
        O200K
            .get_or_init(|| tiktoken_rs::o200k_base().ok())
            .as_ref()
    } else if m.starts_with("gpt-4") || m.starts_with("gpt-3.5") {
        static CL100K: OnceLock<Option<CoreBPE>> = OnceLock::new();
        CL100K
            .get_or_init(|| tiktoken_rs::cl100k_base().ok())
            .as_ref()
    } else {
        None
    };

    match bpe {
        Some(bpe) => bpe.encode_ordinary(text).len() as u64,
        None => (text.chars().count() / 4) as u64,
    }
}

fn phase_cost(model: &str, in_tok: u64, out_tok: u64) -> f64 {
    match lookup_pricing(model) {
        None => 0.0,
        Some(p) => {
            in_tok as f64 * p.input_per_1m / 1_000_000.0
                + out_tok as f64 * p.output_per_1m / 1_000_000.0
        }
    }
}

/// Cost estimate for each phase. No LLM calls are made.
pub fn estimate_pipeline(
    phases: &[PhaseConfig],
    idea: &str,
    _template_name: &str,
    _repo_root: &Path,
    llm: &LLMConfig,
) -> Vec<PhaseEstimate> {
    let mut results = Vec::with_capacity(phases.len());

    for phase in phases {
        if matches!(phase.phase_type, PhaseType::Script | PhaseType::Human) {
            results.push(PhaseEstimate {
                phase: phase.name.clone(),
                phase_type: phase.phase_type,
                model: String::new(),
                in_tok: 0,
                out_tok_mid: 0,
                cost_low: 0.0,
                cost_high: 0.0,
            });
            continue;
        }

        let model = if phase.model.as_deref().unwrap_or("").is_empty() {
            llm.model.clone()
        } else {
            phase.model.clone().unwrap()
        };

        // A representative stub of the prompt (the real prompt template isn't
        // rendered here) — enough for a ballpark input-token count.
        let rendered = format!("You are working on: {idea}\nPhase: {}", phase.name);

        let in_tok = count_tokens(&model, &rendered);
        let mut out_mid = out_heuristic(&phase.name);
        let out_low = (out_mid / 2).max(1);
        let mut out_high = out_mid * 2;

        let (thinking_enabled, thinking_budget) = resolve_thinking(phase, llm);
        if thinking_enabled && model_supports_thinking(&model) {
            out_mid += thinking_budget as u64;
            out_high += thinking_budget as u64;
        }

        results.push(PhaseEstimate {
            phase: phase.name.clone(),
            phase_type: phase.phase_type,
            model: model.clone(),
            in_tok,
            out_tok_mid: out_mid,
            cost_low: phase_cost(&model, in_tok, out_low),
            cost_high: phase_cost(&model, in_tok, out_high),
        });
    }

    results
}
