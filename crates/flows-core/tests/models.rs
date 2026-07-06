//! Ports of the model-relevant cases from the Python test suite (test_models
//! behavior is exercised via test_config there; these cover the types directly).

use flows_core::models::{resolve_preset, resolve_tools, PhaseType};
use flows_core::{Artifact, LLMConfig, PhaseConfig, ToolsConfig, WebSearchConfig};

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
    assert_eq!(p.on_failure, "stop");
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
    assert!(t.bash.is_none());
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
