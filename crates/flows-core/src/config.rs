//! Port of `flows/config.py` — load global (`~/.flows/config.yml`) and project
//! (`.flows.yml`) config, the user-project override, and the 3-tier merge.
//!
//! Raw YAML is parsed to `serde_yaml_ng::Value`, env-expanded, then read
//! field-by-field so the `None = inherit` / empty-string-falls-through semantics
//! match the Python implementation exactly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::models::{
    resolve_profile, resolve_tools, ContextConfig, GlobalConfig, GlobalToolsConfig, LLMConfig,
    ModelError, NotificationsConfig, PhaseConfig, PhaseType, ProjectConfig, WebSearchConfig,
};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Phase config missing required field 'name'")]
    MissingName,
    #[error("Phase {name:?}: max_loop_iterations must be an integer, got {got}")]
    BadMaxLoop { name: String, got: String },
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("failed reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("failed parsing YAML in {path}: {source}")]
    Yaml {
        path: String,
        source: serde_yaml_ng::Error,
    },
}

type Result<T> = std::result::Result<T, ConfigError>;

// ---------------------------------------------------------------------------
// Env expansion  (${VAR} → env value, or left literal if unset)
// ---------------------------------------------------------------------------

fn expand_env_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find('}') {
            let name = &after[..end];
            match std::env::var(name) {
                Ok(v) => out.push_str(&v),
                Err(_) => {
                    out.push_str("${");
                    out.push_str(name);
                    out.push('}');
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push_str("${");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Recursively expand env vars in string and mapping values. Sequences are left
/// untouched — matches Python's `_expand_dict` (which only recurses str + dict).
fn expand_value(v: &mut Value) {
    match v {
        Value::String(s) => *s = expand_env_str(s),
        Value::Mapping(m) => {
            for (_k, val) in m.iter_mut() {
                expand_value(val);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Value accessors
// ---------------------------------------------------------------------------

fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

fn get_bool(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

fn get_u32(v: &Value, key: &str) -> Option<u32> {
    v.get(key)
        .and_then(|x| x.as_u64().or_else(|| x.as_i64().map(|n| n as u64)))
        .map(|n| n as u32)
}

fn get_str_list(v: &Value, key: &str) -> Option<Vec<String>> {
    match v.get(key) {
        Some(Value::Sequence(seq)) => Some(
            seq.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
        ),
        _ => None,
    }
}

fn value_as_int(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(n) = v.as_u64() {
        return Some(n as i64);
    }
    if let Some(f) = v.as_f64() {
        return Some(f as i64); // Python int(float) truncates
    }
    if let Some(s) = v.as_str() {
        if let Ok(n) = s.trim().parse::<i64>() {
            return Some(n);
        }
    }
    None
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// API key from the environment: `ANTHROPIC_API_KEY`, else `OPENAI_API_KEY`
/// (so OpenAI-cloud users don't have to duplicate the key into config).
fn env_api_key() -> String {
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .unwrap_or_default()
}

/// Parse a `models:` mapping into named profiles. Values are already
/// env-expanded (so `api_key: ${FOO}` is resolved); an unset profile `api_key`
/// stays empty here — the dialect-default env fallback is applied at resolution
/// ([`resolve_llm`]), not at load, so it can pick the right key per model.
fn parse_profiles(raw: &Value) -> HashMap<String, LLMConfig> {
    let mut out = HashMap::new();
    if let Some(Value::Mapping(m)) = raw.get("models") {
        for (k, v) in m {
            let Some(name) = k.as_str() else { continue };
            out.insert(
                name.to_string(),
                LLMConfig {
                    model: get_str(v, "model").unwrap_or_default(),
                    api_base: get_str(v, "api_base").unwrap_or_default(),
                    api_key: get_str(v, "api_key").unwrap_or_default(),
                    thinking: get_bool(v, "thinking"),
                    thinking_budget_tokens: get_u32(v, "thinking_budget_tokens"),
                    suites: get_str_list(v, "suites").unwrap_or_default(),
                    supports_tools: get_bool(v, "supports_tools"),
                },
            );
        }
    }
    out
}

/// Dialect-default API key from the environment for a given model: Anthropic's
/// key for `claude*` / `anthropic/*`, otherwise OpenAI's. Used as the per-profile
/// key fallback when a profile sets no explicit `api_key` (D5).
fn dialect_env_key(model: &str) -> String {
    let m = model.to_lowercase();
    let anthropic = m.contains("claude") || m.starts_with("anthropic/");
    let var = if anthropic {
        "ANTHROPIC_API_KEY"
    } else {
        "OPENAI_API_KEY"
    };
    std::env::var(var).unwrap_or_default()
}

fn read_yaml(path: &Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut value: Value = serde_yaml_ng::from_str(&text).map_err(|source| ConfigError::Yaml {
        path: path.display().to_string(),
        source,
    })?;
    if value.is_null() {
        value = Value::Mapping(Default::default());
    }
    expand_value(&mut value);
    Ok(Some(value))
}

// ---------------------------------------------------------------------------
// Global config
// ---------------------------------------------------------------------------

pub fn load_global_config(path: Option<&Path>) -> Result<GlobalConfig> {
    let config_path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home_dir().join(".flows").join("config.yml"));
    let raw = read_yaml(&config_path)?.unwrap_or_else(|| Value::Mapping(Default::default()));

    let env_key = env_api_key();
    let llm_raw = raw.get("llm").cloned().unwrap_or(Value::Null);
    let llm = LLMConfig {
        model: get_str(&llm_raw, "model").unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
        api_base: get_str(&llm_raw, "api_base").unwrap_or_default(),
        api_key: get_str(&llm_raw, "api_key").unwrap_or(env_key),
        thinking: get_bool(&llm_raw, "thinking"),
        thinking_budget_tokens: get_u32(&llm_raw, "thinking_budget_tokens"),
        suites: get_str_list(&llm_raw, "suites").unwrap_or_default(),
        supports_tools: get_bool(&llm_raw, "supports_tools"),
    };

    let notif_raw = raw.get("notifications").cloned().unwrap_or(Value::Null);
    let notifications = NotificationsConfig {
        enabled: get_bool(&notif_raw, "enabled").unwrap_or(true),
        sound: get_bool(&notif_raw, "sound").unwrap_or(false),
    };

    let ws_raw = raw
        .get("tools")
        .and_then(|t| t.get("web_search"))
        .cloned()
        .unwrap_or(Value::Null);
    let web_search = WebSearchConfig {
        backend: get_str(&ws_raw, "backend").unwrap_or_else(|| "duckduckgo".to_string()),
        max_results: get_u32(&ws_raw, "max_results").unwrap_or(5),
        api_key: get_str(&ws_raw, "api_key").unwrap_or_default(),
    };

    Ok(GlobalConfig {
        llm,
        models: parse_profiles(&raw),
        notifications,
        tools: GlobalToolsConfig { web_search },
        max_cost_usd: raw
            .get("max_cost_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    })
}

// ---------------------------------------------------------------------------
// Phase parsing
// ---------------------------------------------------------------------------

pub fn parse_phases(raw: &[Value]) -> Result<Vec<PhaseConfig>> {
    raw.iter().map(parse_phase).collect()
}

/// Parse a template/pipeline YAML document's `phases:` list. Env-expands first,
/// matching how templates and `.flows.yml` are loaded.
pub fn parse_phases_from_yaml(text: &str) -> Result<Vec<PhaseConfig>> {
    let mut root: Value = serde_yaml_ng::from_str(text).map_err(|source| ConfigError::Yaml {
        path: "<template>".to_string(),
        source,
    })?;
    if root.is_null() {
        return Ok(Vec::new());
    }
    expand_value(&mut root);
    match root.get("phases") {
        Some(Value::Sequence(seq)) => parse_phases(seq),
        _ => Ok(Vec::new()),
    }
}

fn parse_phase(p: &Value) -> Result<PhaseConfig> {
    let name = get_str(p, "name").ok_or(ConfigError::MissingName)?;

    let max_loop = match p.get("max_loop_iterations") {
        None | Some(Value::Null) => 3,
        Some(v) => value_as_int(v).ok_or_else(|| ConfigError::BadMaxLoop {
            name: name.clone(),
            got: serde_yaml_ng::to_string(v)
                .unwrap_or_default()
                .trim()
                .to_string(),
        })? as u32,
    };

    let phase_type = match get_str(p, "type").as_deref() {
        Some("script") => PhaseType::Script,
        Some("human") => PhaseType::Human,
        _ => PhaseType::Agent,
    };

    let tools = resolve_tools(p.get("tools"))?;

    Ok(PhaseConfig {
        name,
        phase_type,
        prompt: get_str(p, "prompt"),
        tools,
        auto_advance: get_bool(p, "auto_advance").unwrap_or(false),
        on_failure: get_str(p, "on_failure").unwrap_or_else(|| "stop".to_string()),
        command: get_str(p, "command"),
        instructions: get_str(p, "instructions"),
        max_clarification_rounds: get_u32(p, "max_clarification_rounds").unwrap_or(3),
        max_tool_rounds: get_u32(p, "max_tool_rounds"),
        max_tokens: get_u32(p, "max_tokens"),
        model: get_str(p, "model"),
        thinking: get_bool(p, "thinking"),
        loop_back_to: get_str(p, "loop_back_to"),
        loop_condition: get_str(p, "loop_condition").unwrap_or_default(),
        max_loop_iterations: max_loop,
        depends_on: get_str_list(p, "depends_on").unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// Project config
// ---------------------------------------------------------------------------

pub fn load_project_config(repo_path: &Path) -> Result<Option<ProjectConfig>> {
    let flows_yml = repo_path.join(".flows.yml");
    let raw = match read_yaml(&flows_yml)? {
        None => return Ok(None),
        Some(v) => v,
    };

    let env_key = env_api_key();
    let llm_raw = raw.get("llm").cloned().unwrap_or(Value::Null);
    let llm = LLMConfig {
        model: get_str(&llm_raw, "model").unwrap_or_default(),
        api_base: get_str(&llm_raw, "api_base").unwrap_or_default(),
        api_key: get_str(&llm_raw, "api_key").unwrap_or(env_key),
        thinking: get_bool(&llm_raw, "thinking"),
        thinking_budget_tokens: get_u32(&llm_raw, "thinking_budget_tokens"),
        suites: get_str_list(&llm_raw, "suites").unwrap_or_default(),
        supports_tools: get_bool(&llm_raw, "supports_tools"),
    };

    let phases = match raw.get("phases") {
        Some(Value::Sequence(seq)) => parse_phases(seq)?,
        _ => Vec::new(),
    };

    let ctx_raw = raw.get("context").cloned().unwrap_or(Value::Null);
    let context = ContextConfig {
        exclude: get_str_list(&ctx_raw, "exclude")
            .unwrap_or_else(|| ContextConfig::default().exclude),
        search_tool: get_bool(&ctx_raw, "search_tool").unwrap_or(false),
        range_reads: get_bool(&ctx_raw, "range_reads").unwrap_or(false),
        slim_tree: get_bool(&ctx_raw, "slim_tree").unwrap_or(false),
        repo_map: get_bool(&ctx_raw, "repo_map").unwrap_or(false),
        budgeting: get_bool(&ctx_raw, "budgeting").unwrap_or(false),
        retrieval: get_bool(&ctx_raw, "retrieval").unwrap_or(false),
    };

    Ok(Some(ProjectConfig {
        llm,
        models: parse_profiles(&raw),
        phases,
        template: get_str(&raw, "template").unwrap_or_default(),
        context,
    }))
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

fn or_str(primary: &str, fallback: &str) -> String {
    if primary.is_empty() {
        fallback.to_string()
    } else {
        primary.to_string()
    }
}

/// Project overrides global; falls back to global for unset fields.
pub fn merge_llm(global: &GlobalConfig, project: &ProjectConfig) -> LLMConfig {
    let g = &global.llm;
    let p = &project.llm;
    LLMConfig {
        model: or_str(&p.model, &g.model),
        api_base: or_str(&p.api_base, &g.api_base),
        api_key: or_str(&p.api_key, &g.api_key),
        thinking: p.thinking.or(g.thinking),
        thinking_budget_tokens: p.thinking_budget_tokens.or(g.thinking_budget_tokens),
        suites: if p.suites.is_empty() {
            g.suites.clone()
        } else {
            p.suites.clone()
        },
        supports_tools: p.supports_tools.or(g.supports_tools),
    }
}

/// Merged model profiles: global profiles overlaid by same-named project
/// profiles (project wins).
pub fn merge_profiles(
    global: &GlobalConfig,
    project: &ProjectConfig,
) -> HashMap<String, LLMConfig> {
    let mut m = global.models.clone();
    for (k, v) in &project.models {
        m.insert(k.clone(), v.clone());
    }
    m
}

/// Resolve a model selection to a fully usable [`LLMConfig`]: profile/literal
/// resolution ([`resolve_profile`]) plus per-profile API-key fallback — an empty
/// resolved key is filled from the model's dialect-default env var (D5). This is
/// the runner-facing resolver; estimation uses [`resolve_profile`] directly (it
/// needs the model, not the key).
pub fn resolve_llm(
    selection: &str,
    base: &LLMConfig,
    profiles: &HashMap<String, LLMConfig>,
) -> LLMConfig {
    let mut eff = resolve_profile(selection, base, profiles);
    if eff.api_key.is_empty() {
        eff.api_key = dialect_env_key(&eff.model);
    }
    eff
}

/// Resolve a named context configuration into a [`ContextConfig`] (feature flags
/// only; `exclude` stays at its default). Grammar: `baseline` (all off), `all`
/// (all on), or one-or-more `+token` segments combined — `+search`, `+range`,
/// `+slim`, `+map`, `+budget`, `+rag`. Any unrecognized token is an error. Used
/// by the eval harness to A/B context configurations by name.
pub fn parse_context_config(name: &str) -> std::result::Result<ContextConfig, String> {
    let name = name.trim();
    let mut cfg = ContextConfig {
        // Feature flags start off; exclude keeps its default.
        ..ContextConfig::default()
    };
    match name {
        "baseline" | "" => return Ok(cfg),
        "all" => {
            cfg.search_tool = true;
            cfg.range_reads = true;
            cfg.slim_tree = true;
            cfg.repo_map = true;
            cfg.budgeting = true;
            cfg.retrieval = true;
            return Ok(cfg);
        }
        _ => {}
    }
    for token in name.split('+').filter(|t| !t.is_empty()) {
        match token {
            "search" => cfg.search_tool = true,
            "range" => cfg.range_reads = true,
            "slim" => cfg.slim_tree = true,
            "map" => cfg.repo_map = true,
            "budget" => cfg.budgeting = true,
            "rag" => cfg.retrieval = true,
            other => return Err(format!("unknown context config token: `{other}`")),
        }
    }
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// User project override  (~/.flows/projects/<repo-name>.yml)
// ---------------------------------------------------------------------------

pub fn load_user_project_override(
    repo_path: &Path,
    flows_dir: Option<&Path>,
) -> Result<Option<Value>> {
    let base = flows_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home_dir().join(".flows"));
    let slug = repo_path
        .canonicalize()
        .unwrap_or_else(|_| repo_path.to_path_buf())
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let override_path = base.join("projects").join(format!("{slug}.yml"));
    read_yaml(&override_path)
}

/// Merge a user project override onto a base global config. Only keys present in
/// the override win; absent keys fall back to base. `.flows.yml` still wins over
/// both (applied later, in merge_llm).
pub fn apply_user_project_override(base: &GlobalConfig, override_val: &Value) -> GlobalConfig {
    if !override_val.is_mapping() {
        return base.clone();
    }

    let llm_raw = override_val.get("llm").cloned().unwrap_or(Value::Null);
    let llm = LLMConfig {
        model: or_str(
            &get_str(&llm_raw, "model").unwrap_or_default(),
            &base.llm.model,
        ),
        api_base: or_str(
            &get_str(&llm_raw, "api_base").unwrap_or_default(),
            &base.llm.api_base,
        ),
        api_key: or_str(
            &get_str(&llm_raw, "api_key").unwrap_or_default(),
            &base.llm.api_key,
        ),
        thinking: get_bool(&llm_raw, "thinking").or(base.llm.thinking),
        thinking_budget_tokens: get_u32(&llm_raw, "thinking_budget_tokens")
            .or(base.llm.thinking_budget_tokens),
        suites: get_str_list(&llm_raw, "suites").unwrap_or_else(|| base.llm.suites.clone()),
        supports_tools: get_bool(&llm_raw, "supports_tools").or(base.llm.supports_tools),
    };

    let notifications = if override_val.get("notifications").is_some() {
        let n = override_val
            .get("notifications")
            .cloned()
            .unwrap_or(Value::Null);
        NotificationsConfig {
            enabled: get_bool(&n, "enabled").unwrap_or(true),
            sound: get_bool(&n, "sound").unwrap_or(base.notifications.sound),
        }
    } else {
        base.notifications.clone()
    };

    let ws_raw = override_val
        .get("tools")
        .and_then(|t| t.get("web_search"))
        .cloned()
        .unwrap_or(Value::Null);
    let web_search = WebSearchConfig {
        backend: or_str(
            &get_str(&ws_raw, "backend").unwrap_or_default(),
            &base.tools.web_search.backend,
        ),
        max_results: get_u32(&ws_raw, "max_results")
            .filter(|&n| n != 0)
            .unwrap_or(base.tools.web_search.max_results),
        api_key: or_str(
            &get_str(&ws_raw, "api_key").unwrap_or_default(),
            &base.tools.web_search.api_key,
        ),
    };

    let max_cost = override_val
        .get("max_cost_usd")
        .and_then(Value::as_f64)
        .filter(|&c| c != 0.0)
        .unwrap_or(base.max_cost_usd);

    // Override profiles overlay the base's (override wins per name).
    let mut models = base.models.clone();
    for (k, v) in parse_profiles(override_val) {
        models.insert(k, v);
    }

    GlobalConfig {
        llm,
        models,
        notifications,
        tools: GlobalToolsConfig { web_search },
        max_cost_usd: max_cost,
    }
}
