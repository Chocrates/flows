#![allow(clippy::field_reassign_with_default)]
//! Ports of the behavior in `tests/test_config.py`.

use std::path::Path;
use std::sync::Mutex;

use flows_core::config::{
    apply_user_project_override, load_global_config, load_project_config,
    load_user_project_override, merge_llm, merge_profiles, resolve_llm, ConfigError,
};
use flows_core::models::{GlobalConfig, LLMConfig, PhaseType, ProjectConfig};
use serde_yaml_ng::Value;
use tempfile::tempdir;

/// Serializes tests that mutate process-global env vars.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, body).unwrap();
    p
}

fn parse_phase_yaml(body: &str) -> Result<flows_core::PhaseConfig, ConfigError> {
    let v: Value = serde_yaml_ng::from_str(body).unwrap();
    flows_core::config::parse_phases(std::slice::from_ref(&v)).map(|mut ps| ps.remove(0))
}

// ---------------------------------------------------------------------------
// Global config
// ---------------------------------------------------------------------------

#[test]
fn global_config_missing_file_uses_defaults() {
    let dir = tempdir().unwrap();
    let g = load_global_config(Some(&dir.path().join("nope.yml"))).unwrap();
    assert_eq!(g.llm.model, "claude-sonnet-4-6");
    assert!(g.notifications.enabled);
    assert_eq!(g.tools.web_search.backend, "duckduckgo");
    assert_eq!(g.max_cost_usd, 0.0);
}

#[test]
fn global_config_reads_fields() {
    let dir = tempdir().unwrap();
    let p = write(
        dir.path(),
        "config.yml",
        "llm:\n  model: claude-opus-4-8\nmax_cost_usd: 5.0\nnotifications:\n  enabled: false\n",
    );
    let g = load_global_config(Some(&p)).unwrap();
    assert_eq!(g.llm.model, "claude-opus-4-8");
    assert_eq!(g.max_cost_usd, 5.0);
    assert!(!g.notifications.enabled);
}

#[test]
fn global_config_api_key_from_env_when_absent() {
    let _lock = ENV_LOCK.lock().unwrap();
    std::env::set_var("ANTHROPIC_API_KEY", "sk-from-env");
    let dir = tempdir().unwrap();
    let p = write(dir.path(), "config.yml", "llm:\n  model: m\n");
    let g = load_global_config(Some(&p)).unwrap();
    std::env::remove_var("ANTHROPIC_API_KEY");
    assert_eq!(g.llm.api_key, "sk-from-env");
}

// ---------------------------------------------------------------------------
// Model profiles
// ---------------------------------------------------------------------------

#[test]
fn global_config_parses_model_profiles() {
    let dir = tempdir().unwrap();
    let p = write(
        dir.path(),
        "config.yml",
        "llm:\n  model: claude-sonnet-4-6\nmodels:\n  reasoner:\n    model: o3\n  coder:\n    model: qwen2.5-coder\n    api_base: http://localhost:8080/v1\n",
    );
    let g = load_global_config(Some(&p)).unwrap();
    assert_eq!(g.models.len(), 2);
    assert_eq!(g.models["reasoner"].model, "o3");
    assert_eq!(g.models["coder"].api_base, "http://localhost:8080/v1");
    // A profile with no explicit api_key stays empty at load (filled at resolve).
    assert_eq!(g.models["coder"].api_key, "");
}

#[test]
fn project_profiles_override_global() {
    let global = GlobalConfig {
        models: [(
            "coder".to_string(),
            LLMConfig {
                model: "global-coder".into(),
                ..LLMConfig::default()
            },
        )]
        .into_iter()
        .collect(),
        ..GlobalConfig::default()
    };
    let project = ProjectConfig {
        llm: LLMConfig {
            model: String::new(),
            ..LLMConfig::default()
        },
        models: [(
            "coder".to_string(),
            LLMConfig {
                model: "project-coder".into(),
                ..LLMConfig::default()
            },
        )]
        .into_iter()
        .collect(),
        phases: vec![],
        template: String::new(),
        context: Default::default(),
    };
    let merged = merge_profiles(&global, &project);
    assert_eq!(merged["coder"].model, "project-coder"); // project wins
}

#[test]
fn resolve_llm_fills_dialect_env_key_for_profile() {
    let _lock = ENV_LOCK.lock().unwrap();
    std::env::set_var("OPENAI_API_KEY", "sk-openai");
    std::env::set_var("ANTHROPIC_API_KEY", "sk-anthropic");

    let base = LLMConfig {
        model: "claude-sonnet-4-6".into(),
        api_key: "sk-anthropic".into(),
        ..LLMConfig::default()
    };
    let mut profiles = std::collections::HashMap::new();
    // OpenAI-dialect profile with no explicit key → OPENAI_API_KEY.
    profiles.insert(
        "coder".to_string(),
        LLMConfig {
            model: "gpt-4o".into(),
            ..LLMConfig::default()
        },
    );
    let eff = resolve_llm("coder", &base, &profiles);

    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");

    assert_eq!(eff.model, "gpt-4o");
    assert_eq!(eff.api_key, "sk-openai"); // dialect-default, not the anthropic base key
}

// ---------------------------------------------------------------------------
// Context feature flags
// ---------------------------------------------------------------------------

#[test]
fn context_config_baseline_all_off() {
    let cfg = flows_core::config::parse_context_config("baseline").unwrap();
    assert!(
        !cfg.search_tool
            && !cfg.range_reads
            && !cfg.slim_tree
            && !cfg.repo_map
            && !cfg.budgeting
            && !cfg.retrieval
    );
    // empty name is also baseline.
    assert_eq!(flows_core::config::parse_context_config("").unwrap(), cfg);
}

#[test]
fn context_config_named_enables_only_selected() {
    let cfg = flows_core::config::parse_context_config("+search+map").unwrap();
    assert!(cfg.search_tool && cfg.repo_map);
    assert!(!cfg.range_reads && !cfg.slim_tree && !cfg.budgeting && !cfg.retrieval);
}

#[test]
fn context_config_all_on_and_unknown_errors() {
    let all = flows_core::config::parse_context_config("all").unwrap();
    assert!(
        all.search_tool
            && all.range_reads
            && all.slim_tree
            && all.repo_map
            && all.budgeting
            && all.retrieval
    );
    assert!(flows_core::config::parse_context_config("+search+bogus").is_err());
}

#[test]
fn project_config_parses_context_flags() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        ".flows.yml",
        "llm:\n  model: gpt-4o\ncontext:\n  search_tool: true\n  repo_map: true\n",
    );
    let cfg = load_project_config(dir.path()).unwrap().unwrap();
    assert!(cfg.context.search_tool && cfg.context.repo_map);
    assert!(!cfg.context.budgeting); // unset → default off
}

// ---------------------------------------------------------------------------
// Phase parsing / validation
// ---------------------------------------------------------------------------

#[test]
fn parse_phase_missing_name_errors() {
    assert!(matches!(
        parse_phase_yaml("type: agent\n"),
        Err(ConfigError::MissingName)
    ));
}

#[test]
fn parse_phase_bad_max_loop_errors() {
    let err = parse_phase_yaml("name: review\nmax_loop_iterations: not-a-number\n").unwrap_err();
    assert!(matches!(err, ConfigError::BadMaxLoop { .. }));
}

#[test]
fn parse_phase_max_tool_rounds_default_none() {
    let p = parse_phase_yaml("name: plan\ntype: agent\n").unwrap();
    assert_eq!(p.max_tool_rounds, None);
}

#[test]
fn parse_phase_max_tool_rounds_explicit() {
    let p = parse_phase_yaml("name: implement\nmax_tool_rounds: 40\n").unwrap();
    assert_eq!(p.max_tool_rounds, Some(40));
}

#[test]
fn parse_phase_script_and_human_types() {
    let s = parse_phase_yaml("name: lint\ntype: script\ncommand: 'ruff check .'\n").unwrap();
    assert_eq!(s.phase_type, PhaseType::Script);
    assert_eq!(s.command.as_deref(), Some("ruff check ."));
    let h = parse_phase_yaml("name: approve\ntype: human\ninstructions: ok?\n").unwrap();
    assert_eq!(h.phase_type, PhaseType::Human);
}

#[test]
fn parse_phase_tools_preset_and_loop() {
    let p = parse_phase_yaml(
        "name: review\ntools: research\nloop_back_to: lint\nmax_loop_iterations: 3\n",
    )
    .unwrap();
    assert!(p.tools.web_search);
    assert_eq!(p.loop_back_to.as_deref(), Some("lint"));
}

// ---------------------------------------------------------------------------
// Project config
// ---------------------------------------------------------------------------

#[test]
fn project_config_absent_returns_none() {
    let dir = tempdir().unwrap();
    assert!(load_project_config(dir.path()).unwrap().is_none());
}

#[test]
fn project_config_phases_and_model_override() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        ".flows.yml",
        "llm:\n  model: claude-sonnet-4-6\nphases:\n  - name: plan\n    type: agent\n    model: claude-haiku-4-5\n",
    );
    let cfg = load_project_config(dir.path()).unwrap().unwrap();
    assert_eq!(cfg.phases.len(), 1);
    assert_eq!(cfg.phases[0].model.as_deref(), Some("claude-haiku-4-5"));
}

#[test]
fn project_config_env_expansion() {
    let _lock = ENV_LOCK.lock().unwrap();
    std::env::set_var("MY_MODEL_XYZ", "claude-opus-4-8");
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        ".flows.yml",
        "llm:\n  model: ${MY_MODEL_XYZ}\nphases: []\n",
    );
    let cfg = load_project_config(dir.path()).unwrap().unwrap();
    std::env::remove_var("MY_MODEL_XYZ");
    assert_eq!(cfg.llm.model, "claude-opus-4-8");
}

#[test]
fn env_expansion_leaves_unset_literal() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        ".flows.yml",
        "template: ${DEFINITELY_UNSET_VAR_123}\nphases: []\n",
    );
    let cfg = load_project_config(dir.path()).unwrap().unwrap();
    assert_eq!(cfg.template, "${DEFINITELY_UNSET_VAR_123}");
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

#[test]
fn merge_llm_project_overrides_and_falls_back() {
    let mut global = GlobalConfig::default();
    global.llm = LLMConfig {
        model: "g-model".into(),
        api_key: "g-key".into(),
        ..LLMConfig::default()
    };
    let project = ProjectConfig {
        llm: LLMConfig {
            model: "p-model".into(),
            api_key: String::new(),
            ..LLMConfig::default()
        },
        models: Default::default(),
        phases: vec![],
        template: String::new(),
        context: Default::default(),
    };
    let merged = merge_llm(&global, &project);
    assert_eq!(merged.model, "p-model"); // project wins
    assert_eq!(merged.api_key, "g-key"); // empty project falls back to global
}

#[test]
fn merge_llm_thinking_tristate() {
    let mut global = GlobalConfig::default();
    global.llm.thinking = Some(true);
    let mut project = ProjectConfig {
        llm: LLMConfig::default(),
        models: Default::default(),
        phases: vec![],
        template: String::new(),
        context: Default::default(),
    };
    // project None → inherit global
    assert_eq!(merge_llm(&global, &project).thinking, Some(true));
    // project explicit false → wins
    project.llm.thinking = Some(false);
    assert_eq!(merge_llm(&global, &project).thinking, Some(false));
}

// ---------------------------------------------------------------------------
// User project override
// ---------------------------------------------------------------------------

#[test]
fn user_project_override_applied_over_base() {
    let dir = tempdir().unwrap();
    let flows_dir = dir.path().join(".flows");
    let repo = dir.path().join("myrepo");
    std::fs::create_dir_all(&repo).unwrap();
    write(
        &flows_dir,
        "projects/myrepo.yml",
        "llm:\n  model: override-model\nmax_cost_usd: 9.0\n",
    );

    let ov = load_user_project_override(&repo, Some(&flows_dir))
        .unwrap()
        .unwrap();
    let base = GlobalConfig::default();
    let merged = apply_user_project_override(&base, &ov);
    assert_eq!(merged.llm.model, "override-model");
    assert_eq!(merged.max_cost_usd, 9.0);
    // absent key falls back to base
    assert!(merged.notifications.enabled);
}

#[test]
fn user_project_override_absent_returns_none() {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let ov = load_user_project_override(&repo, Some(&dir.path().join(".flows"))).unwrap();
    assert!(ov.is_none());
}

#[test]
fn parses_notification_sound() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.yml");
    std::fs::write(&p, "notifications:\n  enabled: true\n  sound: true\n").unwrap();
    let g = flows_core::config::load_global_config(Some(&p)).unwrap();
    assert!(g.notifications.enabled);
    assert!(g.notifications.sound);
}

#[test]
fn falls_back_to_openai_api_key_env() {
    let _lock = ENV_LOCK.lock().unwrap();
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::set_var("OPENAI_API_KEY", "sk-openai-xyz");
    let dir = tempdir().unwrap();
    let p = write(dir.path(), "config.yml", "llm:\n  model: gpt-4o\n");
    let g = load_global_config(Some(&p)).unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    assert_eq!(g.llm.api_key, "sk-openai-xyz");
}

#[test]
fn bad_on_failure_rejected_at_load() {
    use flows_core::config::parse_phases_from_yaml;
    let yaml = "phases:\n  - name: build\n    type: agent\n    on_failure: bogus\n";
    let err = parse_phases_from_yaml(yaml).unwrap_err().to_string();
    assert!(err.contains("on_failure"), "should name the field: {err}");
    // Valid forms still parse.
    for ok in ["stop", "continue", "retry:implement"] {
        let y = format!("phases:\n  - name: p\n    type: agent\n    on_failure: \"{ok}\"\n");
        assert!(
            parse_phases_from_yaml(&y).is_ok(),
            "should accept {ok}: {:?}",
            parse_phases_from_yaml(&y).err()
        );
    }
}
