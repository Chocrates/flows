#![allow(clippy::field_reassign_with_default)]
//! Ports of the behavior in `tests/test_estimate.py`.

use std::path::Path;

use flows_core::estimate::estimate_pipeline;
use flows_core::models::{LLMConfig, PhaseConfig, PhaseType};
use flows_core::thinking::{model_supports_thinking, resolve_thinking, DEFAULT_THINKING_BUDGET};

fn no_profiles() -> std::collections::HashMap<String, LLMConfig> {
    Default::default()
}

fn agent(name: &str) -> PhaseConfig {
    PhaseConfig::new(name)
}

fn script(name: &str) -> PhaseConfig {
    let mut p = PhaseConfig::new(name);
    p.phase_type = PhaseType::Script;
    p
}

#[test]
fn script_and_human_phases_are_free() {
    let mut human = PhaseConfig::new("approve");
    human.phase_type = PhaseType::Human;
    let phases = vec![script("lint"), human];
    let est = estimate_pipeline(
        &phases,
        "idea",
        "feature",
        Path::new("/tmp"),
        &LLMConfig::default(),
        &no_profiles(),
    );
    assert_eq!(est.len(), 2);
    assert!(est
        .iter()
        .all(|e| e.cost_low == 0.0 && e.cost_high == 0.0 && e.model.is_empty()));
}

#[test]
fn agent_phase_has_cost_range() {
    let phases = vec![agent("plan")];
    let llm = LLMConfig::default(); // claude-sonnet-4-6
    let est = estimate_pipeline(
        &phases,
        "build a thing",
        "feature",
        Path::new("/tmp"),
        &llm,
        &no_profiles(),
    );
    let e = &est[0];
    assert_eq!(e.model, "claude-sonnet-4-6");
    assert!(e.in_tok > 0);
    assert!(e.cost_high >= e.cost_low);
    assert!(e.cost_low > 0.0);
}

#[test]
fn phase_model_override_used() {
    let mut p = agent("plan");
    p.model = Some("gpt-4o".into());
    let est = estimate_pipeline(
        &[p],
        "idea",
        "feature",
        Path::new("/tmp"),
        &LLMConfig::default(),
        &no_profiles(),
    );
    assert_eq!(est[0].model, "gpt-4o");
}

#[test]
fn mixed_model_plan_prices_each_phase_by_its_profile() {
    // design → reasoner profile (o3), implement → coder profile (local, $0).
    let mut design = agent("design");
    design.model = Some("reasoner".into());
    let mut implement = agent("implement");
    implement.model = Some("coder".into());

    let mut profiles = std::collections::HashMap::new();
    profiles.insert(
        "reasoner".to_string(),
        LLMConfig {
            model: "o3".into(),
            ..LLMConfig::default()
        },
    );
    profiles.insert(
        "coder".to_string(),
        LLMConfig {
            model: "qwen2.5-coder".into(),
            api_base: "http://localhost:8080/v1".into(),
            ..LLMConfig::default()
        },
    );

    let est = estimate_pipeline(
        &[design, implement],
        "idea",
        "feature",
        Path::new("/tmp"),
        &LLMConfig::default(),
        &profiles,
    );
    assert_eq!(est[0].model, "o3");
    assert!(est[0].cost_high > 0.0); // o3 is priced
    assert_eq!(est[1].model, "qwen2.5-coder");
    assert_eq!(est[1].cost_high, 0.0); // local model → $0
}

#[test]
fn thinking_budget_raises_output_estimate() {
    // A thinking-capable model with thinking on should have a larger out_tok_mid
    // than the plain heuristic for the phase.
    let phases = vec![agent("plan")];
    let mut llm = LLMConfig::default();
    llm.thinking = Some(true);
    let with = estimate_pipeline(
        &phases,
        "idea",
        "feature",
        Path::new("/tmp"),
        &llm,
        &no_profiles(),
    );

    let mut llm_off = LLMConfig::default();
    llm_off.thinking = Some(false);
    let without = estimate_pipeline(
        &phases,
        "idea",
        "feature",
        Path::new("/tmp"),
        &llm_off,
        &no_profiles(),
    );

    assert!(with[0].out_tok_mid > without[0].out_tok_mid);
    assert_eq!(
        with[0].out_tok_mid - without[0].out_tok_mid,
        DEFAULT_THINKING_BUDGET as u64
    );
}

#[test]
fn unknown_model_zero_cost() {
    let mut llm = LLMConfig::default();
    llm.model = "made-up".into();
    let est = estimate_pipeline(
        &[agent("plan")],
        "idea",
        "feature",
        Path::new("/tmp"),
        &llm,
        &no_profiles(),
    );
    assert_eq!(est[0].cost_low, 0.0);
    assert_eq!(est[0].cost_high, 0.0);
}

#[test]
fn resolve_thinking_precedence() {
    let mut llm = LLMConfig::default();
    llm.thinking = Some(false);
    let mut phase = PhaseConfig::new("p");
    // phase None → inherit llm(false)
    assert!(!resolve_thinking(&phase, &llm).0);
    // phase Some(true) overrides
    phase.thinking = Some(true);
    assert!(resolve_thinking(&phase, &llm).0);
    // default budget
    assert_eq!(resolve_thinking(&phase, &llm).1, DEFAULT_THINKING_BUDGET);
}

#[test]
fn model_supports_thinking_heuristic() {
    assert!(model_supports_thinking("claude-sonnet-4-6"));
    assert!(model_supports_thinking("claude-opus-4-8"));
    assert!(model_supports_thinking("o3-mini"));
    assert!(model_supports_thinking("openai/o1"));
    assert!(!model_supports_thinking("gpt-4o"));
    assert!(!model_supports_thinking("claude-3-haiku"));
}

#[test]
fn count_tokens_uses_bpe_for_openai_heuristic_for_local() {
    use flows_core::estimate::count_tokens;
    // A string where BPE and chars/4 clearly differ.
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(10);
    let chars4 = (text.chars().count() / 4) as u64;

    // OpenAI models → real tokenizer (not exactly chars/4).
    let gpt = count_tokens("gpt-4o", &text);
    assert!(gpt > 0);
    assert_ne!(gpt, chars4);

    // Local / Anthropic → chars/4 heuristic.
    assert_eq!(count_tokens("llama-3-8b-instruct", &text), chars4);
    assert_eq!(count_tokens("claude-sonnet-4-6", &text), chars4);
}
