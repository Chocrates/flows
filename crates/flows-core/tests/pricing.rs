use flows_core::pricing::{calculate_cost, format_cost, lookup_pricing, tokens_per_second};

#[test]
fn exact_match() {
    let p = lookup_pricing("claude-sonnet-4-6").unwrap();
    assert_eq!(p.input_per_1m, 3.00);
    assert_eq!(p.output_per_1m, 15.00);
}

#[test]
fn strips_provider_prefix() {
    let p = lookup_pricing("anthropic/claude-opus-4-8").unwrap();
    assert_eq!(p.input_per_1m, 15.00);
}

#[test]
fn longest_prefix_match_for_versioned_name() {
    // versioned/dated suffix still resolves to the base model
    let p = lookup_pricing("claude-sonnet-4-6-20250219").unwrap();
    assert_eq!(p.output_per_1m, 15.00);
}

#[test]
fn unknown_model_is_none() {
    assert!(lookup_pricing("totally-made-up-model").is_none());
}

#[test]
fn local_models_are_free() {
    assert_eq!(lookup_pricing("ollama/llama3").unwrap().input_per_1m, 0.0);
    assert_eq!(lookup_pricing("local-whatever").unwrap().output_per_1m, 0.0);
}

#[test]
fn calculate_cost_known_model() {
    // 1M input @ $3, 1M output @ $15 = $18
    let c = calculate_cost("claude-sonnet-4-6", 1_000_000, 1_000_000);
    assert!((c - 18.0).abs() < 1e-9);
}

#[test]
fn calculate_cost_unknown_is_zero() {
    assert_eq!(calculate_cost("mystery", 1000, 1000), 0.0);
}

#[test]
fn tokens_per_second_zero_latency() {
    assert_eq!(tokens_per_second(100, 0), 0.0);
}

#[test]
fn tokens_per_second_basic() {
    assert!((tokens_per_second(100, 1000) - 100.0).abs() < 1e-9);
}

#[test]
fn format_cost_tiers() {
    assert_eq!(format_cost(0.0), "$0.000000");
    assert_eq!(format_cost(0.0001234), "$0.000123");
    assert_eq!(format_cost(1.23456), "$1.2346");
}
