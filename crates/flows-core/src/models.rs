//! Port of `flows/models.py` — config, run-state, and artifact data types.
//!
//! Serde derives keep the on-disk JSON/YAML shape identical to the Python
//! implementation (state.json, meta.json, artifacts/*.yml, llm.jsonl).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("Unknown tool preset: {0:?}. Valid: {valid:?}", valid = TOOL_PRESET_NAMES)]
    UnknownPreset(String),
    #[error("Invalid tools config: expected preset name or mapping, got {0}")]
    InvalidTools(String),
}

// ---------------------------------------------------------------------------
// Tool config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BashConfig {
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub unrestricted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsConfig {
    #[serde(default = "default_true")]
    pub files: bool,
    /// Read-only file access: `read_file`/`list_files` are offered, but
    /// `write_file`/`make_dir` are withheld (for planning/review phases that
    /// must not modify the repo).
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub bash: Option<BashConfig>,
    #[serde(default)]
    pub http_get: bool,
    #[serde(default)]
    pub web_search: bool,
    #[serde(default)]
    pub plugins: Vec<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            files: true,
            read_only: false,
            bash: None,
            http_get: false,
            web_search: false,
            plugins: Vec::new(),
        }
    }
}

pub const TOOL_PRESET_NAMES: &[&str] = &[
    "files",
    "files_and_test",
    "files_and_deps",
    "research",
    "research_ro",
    "research_and_code",
    "all",
];

fn bash_allow(patterns: &[&str]) -> Option<BashConfig> {
    Some(BashConfig {
        allowed: patterns.iter().map(|s| s.to_string()).collect(),
        unrestricted: false,
    })
}

/// Look up a named tool preset, mirroring `TOOL_PRESETS` in models.py.
pub fn resolve_preset(name: &str) -> Option<ToolsConfig> {
    let t = |bash| ToolsConfig {
        bash,
        ..ToolsConfig::default()
    };
    Some(match name {
        "files" => ToolsConfig::default(),
        "files_and_test" => t(bash_allow(&[
            "make*", // preferred: projects expose tooling via make (see AGENTS.md)
            "pytest*",
            "python -m pytest*",
            "npm test*",
            "yarn test*",
            "cargo test*",
            "go test*",
            "ruff check*",
            "mypy*",
        ])),
        "files_and_deps" => t(bash_allow(&[
            "pip install*",
            "uv add*",
            "uv pip install*",
            "npm install*",
            "yarn add*",
            "pnpm install*",
            "brew install*",
            "cargo add*",
        ])),
        "research" => ToolsConfig {
            http_get: true,
            web_search: true,
            ..ToolsConfig::default()
        },
        // Read-only research: inspect files + research, but cannot write (for
        // plan / spec / design phases that must not modify the repo).
        "research_ro" => ToolsConfig {
            http_get: true,
            web_search: true,
            read_only: true,
            ..ToolsConfig::default()
        },
        "research_and_code" => ToolsConfig {
            http_get: true,
            web_search: true,
            bash: bash_allow(&["pytest*", "pip install*", "uv*", "make*", "npm*"]),
            ..ToolsConfig::default()
        },
        "all" => ToolsConfig {
            http_get: true,
            web_search: true,
            bash: Some(BashConfig {
                allowed: Vec::new(),
                unrestricted: true,
            }),
            ..ToolsConfig::default()
        },
        _ => return None,
    })
}

fn as_bool(v: Option<&Value>) -> Option<bool> {
    v.and_then(Value::as_bool)
}

fn as_str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Resolve the `tools:` field of a phase, which may be absent, a preset name,
/// or an inline mapping. Mirrors `resolve_tools` in models.py.
pub fn resolve_tools(raw: Option<&Value>) -> Result<ToolsConfig, ModelError> {
    match raw {
        None | Some(Value::Null) => Ok(ToolsConfig::default()),
        Some(Value::String(s)) => {
            resolve_preset(s).ok_or_else(|| ModelError::UnknownPreset(s.clone()))
        }
        Some(Value::Mapping(m)) => {
            let bash = match m.get("bash") {
                Some(Value::Bool(true)) => Some(BashConfig {
                    allowed: Vec::new(),
                    unrestricted: true,
                }),
                Some(bm @ Value::Mapping(_)) => Some(BashConfig {
                    allowed: as_str_list(bm.get("allowed")),
                    unrestricted: as_bool(bm.get("unrestricted")).unwrap_or(false),
                }),
                _ => None,
            };
            Ok(ToolsConfig {
                files: as_bool(m.get("files")).unwrap_or(true),
                read_only: as_bool(m.get("read_only")).unwrap_or(false),
                bash,
                http_get: as_bool(m.get("http_get")).unwrap_or(false),
                web_search: as_bool(m.get("web_search")).unwrap_or(false),
                plugins: as_str_list(m.get("plugins")),
            })
        }
        Some(other) => Err(ModelError::InvalidTools(
            serde_yaml_ng::to_string(other)
                .unwrap_or_default()
                .trim()
                .to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Phase config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PhaseType {
    #[default]
    Agent,
    Script,
    Human,
}

impl PhaseType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PhaseType::Agent => "agent",
            PhaseType::Script => "script",
            PhaseType::Human => "human",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseConfig {
    pub name: String,
    #[serde(rename = "type", default)]
    pub phase_type: PhaseType,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub auto_advance: bool,
    #[serde(default = "default_on_failure")]
    pub on_failure: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default = "default_three")]
    pub max_clarification_rounds: u32,
    #[serde(default)]
    pub max_tool_rounds: Option<u32>,
    /// Max completion tokens per model call. Raise for reasoning models (they emit
    /// long hidden reasoning). None → the agent default.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<bool>,
    #[serde(default)]
    pub loop_back_to: Option<String>,
    #[serde(default)]
    pub loop_condition: String,
    #[serde(default = "default_three")]
    pub max_loop_iterations: u32,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl PhaseConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            phase_type: PhaseType::Agent,
            prompt: None,
            tools: ToolsConfig::default(),
            auto_advance: false,
            on_failure: default_on_failure(),
            command: None,
            instructions: None,
            max_clarification_rounds: 3,
            max_tool_rounds: None,
            max_tokens: None,
            model: None,
            thinking: None,
            loop_back_to: None,
            loop_condition: String::new(),
            max_loop_iterations: 3,
            depends_on: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// LLM + context config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LLMConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub api_base: String,
    #[serde(default)]
    pub api_key: String,
    /// None = not set at this layer (falls through to the next config layer).
    #[serde(default)]
    pub thinking: Option<bool>,
    #[serde(default)]
    pub thinking_budget_tokens: Option<u32>,
    /// Eval-only: the fixture suites this model is intended for (`coder` /
    /// `reasoner`). When set on a profile, `flows eval` restricts that model to
    /// those suites. Empty = no restriction. Ignored outside eval.
    #[serde(default)]
    pub suites: Vec<String>,
    /// Whether the model supports tool/function calling. `Some(false)` marks a
    /// model that can't call tools (e.g. many reasoning models): the runner then
    /// sends no tool schemas and instead expects the model to emit any files
    /// inline in its artifact, which the runner writes. `None` = assume it can.
    #[serde(default)]
    pub supports_tools: Option<bool>,
}

impl Default for LLMConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            api_base: String::new(),
            api_key: String::new(),
            thinking: None,
            thinking_budget_tokens: None,
            suites: Vec::new(),
            supports_tools: None,
        }
    }
}

/// Resolve a model selection (a `phase.model` / `llm.model` string) against the
/// configured profiles and a base `LLMConfig`. Returns the effective config.
///
/// Precedence (D2): an empty selection uses the base model as the selection; a
/// selection that names a profile yields that profile merged over the base
/// (profile fields win where set); otherwise the selection is a literal model on
/// the base endpoint (today's backward-compatible behavior). The `api_key` here
/// is the profile's explicit key (possibly empty) for a profile match, or the
/// base key for a literal — env-key fallback is applied by the config layer.
pub fn resolve_profile(
    selection: &str,
    base: &LLMConfig,
    profiles: &HashMap<String, LLMConfig>,
) -> LLMConfig {
    let sel = if selection.is_empty() {
        base.model.as_str()
    } else {
        selection
    };
    match profiles.get(sel) {
        Some(p) => LLMConfig {
            model: if p.model.is_empty() {
                base.model.clone()
            } else {
                p.model.clone()
            },
            api_base: if p.api_base.is_empty() {
                base.api_base.clone()
            } else {
                p.api_base.clone()
            },
            api_key: p.api_key.clone(),
            thinking: p.thinking.or(base.thinking),
            thinking_budget_tokens: p.thinking_budget_tokens.or(base.thinking_budget_tokens),
            suites: p.suites.clone(),
            supports_tools: p.supports_tools.or(base.supports_tools),
        },
        None => LLMConfig {
            model: sel.to_string(),
            ..base.clone()
        },
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextConfig {
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
    /// Context-delivery feature flags. All default off = the pre-change baseline.
    /// These gate the planned context features (search, repo map, budgeting,
    /// retrieval); they are configurable + selectable now (e.g. by the eval
    /// harness), but no runner/tool reads them until those features land.
    #[serde(default)]
    pub search_tool: bool,
    #[serde(default)]
    pub range_reads: bool,
    #[serde(default)]
    pub slim_tree: bool,
    #[serde(default)]
    pub repo_map: bool,
    #[serde(default)]
    pub budgeting: bool,
    #[serde(default)]
    pub retrieval: bool,
    /// Oracle push (eval-only stopgap): inject a labeled set of file contents up
    /// front. The eval harness sets `FLOWS_ORACLE_CONTEXT` from a fixture's
    /// `relevant` files; the runner injects it so a no-tool model has the code
    /// without needing to pull it. Outside eval there is no source, so it no-ops.
    #[serde(default)]
    pub oracle: bool,
    /// Local embedding model for `retrieval` (fastembed / ONNX, run in-process).
    /// Empty → a small default (all-MiniLM-L6-v2). Accepts e.g. `bge-small`,
    /// `bge-base`. Downloaded once to the fastembed cache on first use.
    #[serde(default)]
    pub retrieval_model: String,
    /// Max retrieved chunks injected when `retrieval` is on.
    #[serde(default = "default_max_chunks")]
    pub retrieval_max_chunks: usize,
}

fn default_max_chunks() -> usize {
    8
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            exclude: default_exclude(),
            search_tool: false,
            range_reads: false,
            slim_tree: false,
            repo_map: false,
            budgeting: false,
            retrieval: false,
            oracle: false,
            retrieval_model: String::new(),
            retrieval_max_chunks: default_max_chunks(),
        }
    }
}

// ---------------------------------------------------------------------------
// Project / global config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub llm: LLMConfig,
    /// Named model profiles (each a full model + endpoint + key + thinking).
    /// Project profiles override same-named global profiles.
    #[serde(default)]
    pub models: HashMap<String, LLMConfig>,
    #[serde(default)]
    pub phases: Vec<PhaseConfig>,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub context: ContextConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebSearchConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_five")]
    pub max_results: u32,
    #[serde(default)]
    pub api_key: String,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            max_results: 5,
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GlobalToolsConfig {
    #[serde(default)]
    pub web_search: WebSearchConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Play a sound with the notification.
    #[serde(default)]
    pub sound: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sound: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub llm: LLMConfig,
    /// Named model profiles (see [`ProjectConfig::models`]).
    #[serde(default)]
    pub models: HashMap<String, LLMConfig>,
    #[serde(default)]
    pub notifications: NotificationsConfig,
    #[serde(default)]
    pub tools: GlobalToolsConfig,
    #[serde(default)]
    pub max_cost_usd: f64,
    /// Context-delivery settings (e.g. `retrieval_model`). Project `context`
    /// overrides these where present.
    #[serde(default)]
    pub context: ContextConfig,
}

// ---------------------------------------------------------------------------
// Run state / artifacts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: String,
    pub idea: String,
    pub slug: String,
    pub repo_path: String,
    pub template: String,
    /// Serialized PhaseConfig values (kept as generic JSON, as in Python).
    pub phases: Vec<serde_json::Value>,
    /// ISO-8601 timestamp. Populated automatically via `now_iso8601()` on new
    /// runs; existing on-disk JSON with `""` deserialises without error.
    #[serde(default = "now_iso8601")]
    pub created_at: String,
    #[serde(default)]
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub run_id: String,
    pub status: String,
    pub current_phase: String,
    pub current_phase_index: i64,
    #[serde(default)]
    pub round: i64,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub completed_phases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub idea: String,
    #[serde(default)]
    pub content: serde_json::Value,
}

impl Default for Artifact {
    fn default() -> Self {
        Self {
            version: default_version(),
            phase: String::new(),
            run_id: String::new(),
            idea: String::new(),
            content: serde_json::Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptResult {
    pub command: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ScriptResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

// ---------------------------------------------------------------------------
// chrono helper — ISO-8601 / RFC-3339 timestamp
// ---------------------------------------------------------------------------

/// Return the current UTC time as an RFC-3339 string, e.g.
/// `"2025-01-15T09:30:00+00:00"`.  Used as the serde `default` for
/// `RunMeta.created_at` so every new meta written to disk gets a real
/// timestamp without changing the field type (still a plain `String`).
pub fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// serde default helpers
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}
fn default_three() -> u32 {
    3
}
fn default_five() -> u32 {
    5
}
fn default_on_failure() -> String {
    "stop".to_string()
}
fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}
fn default_backend() -> String {
    "duckduckgo".to_string()
}
fn default_version() -> String {
    "1".to_string()
}

fn default_exclude() -> Vec<String> {
    [
        "*.lock",
        ".venv/",
        "node_modules/",
        "__pycache__/",
        ".git/",
        "dist/",
        "build/",
        ".mypy_cache/",
        ".pytest_cache/",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
