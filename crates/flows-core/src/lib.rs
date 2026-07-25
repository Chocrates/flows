//! flows-core — data models and (soon) config/state/templates for the Rust port
//! of `flows`. Pure logic, no I/O beyond the filesystem.

pub mod config;
pub mod detect;
pub mod dryrun;
pub mod estimate;
pub mod json;
pub mod llm_log;
pub mod models;
pub mod notify;
pub mod pipeline;
pub mod pricing;
pub mod state;
pub mod templates;
pub mod thinking;
pub mod usage;

pub use dryrun::{build_plan_lines, dry_run, validate_dry_run, DryRunReport};
pub use estimate::{estimate_pipeline, PhaseEstimate};
pub use llm_log::LlmLogger;
pub use pipeline::{build_deps, is_dag, validate_pipeline};
pub use thinking::{model_supports_thinking, resolve_thinking, DEFAULT_THINKING_BUDGET};

pub use state::{StateError, Store};
pub use templates::{list_templates, load_template, TemplateError};
pub use usage::{parse_usage, PhaseUsage, UsageReport};

pub use config::{
    apply_user_project_override, load_global_config, load_project_config,
    load_user_project_override, merge_llm, parse_phases, ConfigError,
};
pub use detect::{detect_stack, generate_config, StackInfo};
pub use pricing::{calculate_cost, format_cost, lookup_pricing, tokens_per_second, ModelPricing};

pub use models::{
    now_iso8601, resolve_preset, resolve_tools, Artifact, BashConfig, ContextConfig, GlobalConfig,
    GlobalToolsConfig, LLMConfig, ModelError, NotificationsConfig, PhaseConfig, PhaseType,
    ProjectConfig, RunMeta, RunState, ScriptResult, ToolsConfig, WebSearchConfig,
    TOOL_PRESET_NAMES,
};
