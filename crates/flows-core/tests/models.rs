//! Ports of the model-relevant cases from the Python test suite (test_models
//! behavior is exercised via test_config there; these cover the types directly).

use std::collections::HashMap;

use flows_core::models::{resolve_preset, resolve_profile, resolve_tools, OnFailure, PhaseType};
use flows_core::{Artifact, LLMConfig, PhaseConfig, ToolsConfig, WebSearchConfig};

fn base() -> LLMConfig {
    LLMConfig {
        model: "claude-sonnet-4-6".into(),
        api_base: String::new(),
        api_key: "base-key".into(),
        thinking: Some(false),
        thinking_budget_tokens: None,
        suites: Vec::new(),
        supports_tools: None,
    }
}

#[test]
fn resolve_profile_matches_named_profile() {
    let mut profiles = HashMap::new();
    profiles.insert(
        "coder".to_string(),
        LLMConfig {
            model: "qwen2.5-coder".into(),
            api_base: "http://localhost:8080/v1".into(),
            api_key: "prof-key".into(),
            thinking: Some(true),
            thinking_budget_tokens: Some(1024),
            suites: Vec::new(),
            supports_tools: None,
        },
    );
    let eff = resolve_profile("coder", &base(), &profiles);
    assert_eq!(eff.model, "qwen2.5-coder");
    assert_eq!(eff.api_base, "http://localhost:8080/v1");
    assert_eq!(eff.api_key, "prof-key");
    assert_eq!(eff.thinking, Some(true));
    assert_eq!(eff.thinking_budget_tokens, Some(1024));
}

#[test]
fn resolve_profile_falls_back_to_literal_model() {
    // A selection that names no profile is a literal model on the base endpoint.
    let eff = resolve_profile("gpt-4o", &base(), &HashMap::new());
    assert_eq!(eff.model, "gpt-4o");
    assert_eq!(eff.api_key, "base-key"); // literal keeps base key/endpoint
    assert_eq!(eff.thinking, Some(false));
}

#[test]
fn resolve_profile_empty_uses_base_model() {
    // Empty selection resolves the base model (which may itself name a profile).
    let eff = resolve_profile("", &base(), &HashMap::new());
    assert_eq!(eff.model, "claude-sonnet-4-6");
}

#[test]
fn resolve_profile_empty_selection_resolves_base_profile_name() {
    let mut profiles = HashMap::new();
    profiles.insert(
        "reasoner".to_string(),
        LLMConfig {
            model: "o3".into(),
            ..LLMConfig::default()
        },
    );
    // base.model is a profile name → empty phase selection routes to it.
    let mut b = base();
    b.model = "reasoner".into();
    let eff = resolve_profile("", &b, &profiles);
    assert_eq!(eff.model, "o3");
}

#[test]
fn resolve_profile_inherits_base_endpoint_when_unset() {
    let mut profiles = HashMap::new();
    profiles.insert(
        "big".to_string(),
        LLMConfig {
            model: "claude-opus-4-8".into(),
            ..LLMConfig::default()
        },
    );
    let mut b = base();
    b.api_base = "https://gw.example/v1".into();
    let eff = resolve_profile("big", &b, &profiles);
    // profile set no endpoint → inherits base endpoint
    assert_eq!(eff.api_base, "https://gw.example/v1");
}

#[test]
fn tools_config_defaults_files_on() {
    let t = ToolsConfig::default();
    assert!(t.files);
    assert!(t.bash.is_none());
    assert!(!t.http_get);
    assert!(!t.web_search);
}

#[test]
fn llm_config_default_model() {
    assert_eq!(LLMConfig::default().model, "claude-sonnet-4-6");
    assert_eq!(LLMConfig::default().thinking, None);
}

#[test]
fn phase_defaults() {
    let p = PhaseConfig::new("plan");
    assert_eq!(p.phase_type, PhaseType::Agent);
    assert_eq!(p.on_failure, OnFailure::Stop);
    assert_eq!(p.max_clarification_rounds, 3);
    assert_eq!(p.max_tool_rounds, None);
    assert_eq!(p.max_loop_iterations, 3);
    assert!(p.depends_on.is_empty());
}

#[test]
fn preset_files_and_test_has_test_commands() {
    let t = resolve_preset("files_and_test").unwrap();
    let bash = t.bash.unwrap();
    assert!(bash.allowed.iter().any(|p| p.starts_with("pytest")));
    assert!(!bash.unrestricted);
}

#[test]
fn preset_research_enables_web() {
    let t = resolve_preset("research").unwrap();
    assert!(t.http_get);
    assert!(t.web_search);
    assert!(t.lookup); // web_search phases also get the structured lookup tool
    assert!(t.bash.is_none());
}

#[test]
fn every_web_search_preset_also_grants_lookup() {
    // Invariant: any phase that can web_search must also have the structured
    // `lookup` tool (crates.io/docs.rs/etc.) — the reliable doc source. web_search
    // is only ever granted via a preset, so enforcing it here covers every phase.
    for name in flows_core::models::TOOL_PRESET_NAMES {
        let t = resolve_preset(name).unwrap();
        if t.web_search {
            assert!(t.lookup, "preset {name:?} has web_search but not lookup");
        }
    }
}

#[test]
fn preset_all_is_unrestricted() {
    let t = resolve_preset("all").unwrap();
    assert!(t.bash.unwrap().unrestricted);
}

#[test]
fn resolve_tools_none_defaults_files() {
    let t = resolve_tools(None).unwrap();
    assert!(t.files);
}

#[test]
fn resolve_tools_string_preset() {
    let v = serde_yaml_ng::from_str::<serde_yaml_ng::Value>("research").unwrap();
    let t = resolve_tools(Some(&v)).unwrap();
    assert!(t.web_search);
}

#[test]
fn resolve_tools_unknown_preset_errors() {
    let v = serde_yaml_ng::from_str::<serde_yaml_ng::Value>("bogus").unwrap();
    assert!(resolve_tools(Some(&v)).is_err());
}

#[test]
fn resolve_tools_inline_mapping_with_bash_allowlist() {
    let yaml = "files: true\nbash:\n  allowed:\n    - \"make*\"\n    - \"rm*\"\nhttp_get: false\n";
    let v = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(yaml).unwrap();
    let t = resolve_tools(Some(&v)).unwrap();
    let bash = t.bash.unwrap();
    assert_eq!(bash.allowed, vec!["make*".to_string(), "rm*".to_string()]);
    assert!(!bash.unrestricted);
}

#[test]
fn resolve_tools_bash_resource_limits() {
    let yaml = "bash:\n  allowed: [\"cargo*\"]\n  memory_mb: 16384\n  max_procs: 0\n";
    let v = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(yaml).unwrap();
    let bash = resolve_tools(Some(&v)).unwrap().bash.unwrap();
    assert_eq!(bash.memory_mb, Some(16384));
    assert_eq!(bash.max_procs, Some(0));
    // Unset → None (sandbox defaults apply).
    let v = serde_yaml_ng::from_str::<serde_yaml_ng::Value>("bash:\n  allowed: []\n").unwrap();
    let bash = resolve_tools(Some(&v)).unwrap().bash.unwrap();
    assert_eq!((bash.memory_mb, bash.max_procs), (None, None));
}

#[test]
fn resolve_tools_bash_true_is_unrestricted() {
    let v = serde_yaml_ng::from_str::<serde_yaml_ng::Value>("bash: true\n").unwrap();
    let t = resolve_tools(Some(&v)).unwrap();
    assert!(t.bash.unwrap().unrestricted);
}

#[test]
fn phase_type_serializes_lowercase() {
    let p = PhaseConfig::new("lint");
    let mut p = p;
    p.phase_type = PhaseType::Script;
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(json["type"], "script");
}

#[test]
fn artifact_round_trips_with_content() {
    let a = Artifact {
        phase: "plan".into(),
        content: serde_json::json!({"summary": "done", "steps": [1, 2, 3]}),
        ..Artifact::default()
    };
    let s = serde_json::to_string(&a).unwrap();
    let back: Artifact = serde_json::from_str(&s).unwrap();
    assert_eq!(back.version, "1");
    assert_eq!(back.content["summary"], "done");
    assert_eq!(a, back);
}

#[test]
fn web_search_config_defaults_duckduckgo() {
    assert_eq!(WebSearchConfig::default().backend, "duckduckgo");
    assert_eq!(WebSearchConfig::default().max_results, 5);
}

#[test]
fn research_ro_preset_is_read_only() {
    let t = flows_core::models::resolve_preset("research_ro").unwrap();
    assert!(t.files);
    assert!(t.read_only);
    assert!(t.http_get && t.web_search);
    // plain research is NOT read-only
    assert!(
        !flows_core::models::resolve_preset("research")
            .unwrap()
            .read_only
    );
}
