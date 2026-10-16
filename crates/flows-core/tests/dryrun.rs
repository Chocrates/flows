//! Ports of behavior in `tests/test_runner.py` (validation) + dryrun.

use std::path::Path;

use flows_core::dryrun::{build_plan_lines, dry_run, validate_dry_run};
use flows_core::models::{GlobalConfig, OnFailure, PhaseConfig, PhaseType, ProjectConfig};
use flows_core::pipeline::validate_pipeline;

fn agent(name: &str) -> PhaseConfig {
    PhaseConfig::new(name)
}

fn empty_project() -> ProjectConfig {
    ProjectConfig {
        llm: flows_core::models::LLMConfig {
            model: String::new(),
            ..Default::default()
        },
        models: Default::default(),
        phases: vec![],
        template: String::new(),
        context: Default::default(),
        context_keys: Vec::new(),
    }
}

// -- validate_pipeline -------------------------------------------------------

#[test]
fn dag_valid() {
    let mut b = agent("b");
    b.depends_on = vec!["a".into()];
    assert!(validate_pipeline(&[agent("a"), b]).is_ok());
}

#[test]
fn dag_unknown_dep_errors() {
    let mut b = agent("b");
    b.depends_on = vec!["nonexistent".into()];
    let err = validate_pipeline(&[agent("a"), b]).unwrap_err();
    assert!(err.contains("not found in pipeline"));
}

#[test]
fn dag_loop_forbidden() {
    let mut b = agent("b");
    b.depends_on = vec!["a".into()];
    b.loop_back_to = Some("a".into());
    let err = validate_pipeline(&[agent("a"), b]).unwrap_err();
    assert!(err.contains("loop_back_to cannot be used with depends_on"));
}

#[test]
fn dag_retry_forbidden() {
    let mut b = agent("b");
    b.depends_on = vec!["a".into()];
    b.on_failure = OnFailure::Retry("a".into());
    let err = validate_pipeline(&[agent("a"), b]).unwrap_err();
    assert!(err.contains("retry:X on_failure not supported in DAG"));
}

#[test]
fn linear_loop_target_must_exist() {
    let mut b = agent("b");
    b.loop_back_to = Some("nope".into());
    let err = validate_pipeline(&[agent("a"), b]).unwrap_err();
    assert!(err.contains("loop_back_to"));
    assert!(err.contains("not found in pipeline"));
}

#[test]
fn linear_valid_loop() {
    let mut review = agent("review");
    review.loop_back_to = Some("implement".into());
    assert!(validate_pipeline(&[agent("implement"), review]).is_ok());
}

// -- plan lines --------------------------------------------------------------

#[test]
fn plan_lines_linear_numbered() {
    let lines = build_plan_lines(&[agent("plan"), agent("implement")]);
    assert!(lines[0].contains("1."));
    assert!(lines[0].contains("plan"));
    assert!(lines[0].contains("[agent]"));
    assert!(lines[1].contains("2."));
}

#[test]
fn plan_lines_dag_shows_deps() {
    let mut b = agent("b");
    b.depends_on = vec!["a".into()];
    let lines = build_plan_lines(&[agent("a"), b]);
    assert!(lines[0].contains("runs immediately"));
    assert!(lines[1].contains("deps: a"));
}

// -- dry_run -----------------------------------------------------------------

#[test]
fn validate_dry_run_flags_missing_model() {
    let global = GlobalConfig {
        llm: flows_core::models::LLMConfig {
            model: String::new(),
            ..Default::default()
        },
        ..Default::default()
    };
    let errors = validate_dry_run(&[agent("plan")], &global, &empty_project());
    assert!(errors.iter().any(|e| e.contains("No LLM model configured")));
}

#[test]
fn dry_run_produces_report_with_estimates() {
    let global = GlobalConfig::default(); // has default model
    let mut script = agent("lint");
    script.phase_type = PhaseType::Script;
    let phases = vec![agent("plan"), script];
    let report = dry_run(
        "build a thing",
        &phases,
        "feature",
        &global,
        &empty_project(),
        Path::new("/tmp"),
    );
    assert!(report.errors.is_empty());
    assert_eq!(report.estimates.len(), 2);
    assert!(report.total_cost_high >= report.total_cost_low);
    assert!(report.total_cost_low > 0.0); // the agent phase costs something
}
