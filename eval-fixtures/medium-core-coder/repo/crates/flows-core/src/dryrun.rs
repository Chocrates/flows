//! Port of `flows/dryrun.py` — validate config, show execution plan + estimate.

use std::path::Path;

use crate::config::{merge_llm, merge_profiles};
use crate::estimate::{estimate_pipeline, PhaseEstimate};
use crate::models::{GlobalConfig, PhaseConfig, ProjectConfig};
use crate::pipeline::{is_dag, validate_pipeline};

/// Human-readable lines describing execution order.
pub fn build_plan_lines(phases: &[PhaseConfig]) -> Vec<String> {
    let mut lines = Vec::with_capacity(phases.len());
    if is_dag(phases) {
        for p in phases {
            let dep_str = if p.depends_on.is_empty() {
                "  deps: (none — runs immediately)".to_string()
            } else {
                format!("  deps: {}", p.depends_on.join(", "))
            };
            lines.push(format!(
                "  {:<20} [{}]{}",
                p.name,
                p.phase_type.as_str(),
                dep_str
            ));
        }
    } else {
        for (i, p) in phases.iter().enumerate() {
            lines.push(format!(
                "  {}. {:<20} [{}]",
                i + 1,
                p.name,
                p.phase_type.as_str()
            ));
        }
    }
    lines
}

/// Validation errors (empty = ok).
pub fn validate_dry_run(
    phases: &[PhaseConfig],
    global_cfg: &GlobalConfig,
    project_cfg: &ProjectConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(e) = validate_pipeline(phases) {
        errors.push(e);
    }
    let model = if project_cfg.llm.model.is_empty() {
        &global_cfg.llm.model
    } else {
        &project_cfg.llm.model
    };
    if model.is_empty() {
        errors.push(
            "No LLM model configured. Set llm.model in ~/.flows/config.yml or .flows.yml".into(),
        );
    }
    errors
}

#[derive(Debug, Clone, PartialEq)]
pub struct DryRunReport {
    pub errors: Vec<String>,
    pub plan_lines: Vec<String>,
    pub estimates: Vec<PhaseEstimate>,
    pub total_cost_low: f64,
    pub total_cost_high: f64,
}

pub fn dry_run(
    idea: &str,
    phases: &[PhaseConfig],
    template_name: &str,
    global_cfg: &GlobalConfig,
    project_cfg: &ProjectConfig,
    repo_root: &Path,
) -> DryRunReport {
    let errors = validate_dry_run(phases, global_cfg, project_cfg);
    let plan_lines = build_plan_lines(phases);

    let llm = merge_llm(global_cfg, project_cfg);
    let profiles = merge_profiles(global_cfg, project_cfg);
    let estimates = estimate_pipeline(phases, idea, template_name, repo_root, &llm, &profiles);
    let total_cost_low = estimates.iter().map(|e| e.cost_low).sum();
    let total_cost_high = estimates.iter().map(|e| e.cost_high).sum();

    DryRunReport {
        errors,
        plan_lines,
        estimates,
        total_cost_low,
        total_cost_high,
    }
}
