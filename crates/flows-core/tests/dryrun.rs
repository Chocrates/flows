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

// -- when conditions ---------------------------------------------------------

#[test]
fn invalid_when_rejected_with_phase_name() {
    let mut b = agent("fix");
    b.when = Some("artifacts.review.verdict ==".into());
    let err = validate_pipeline(&[agent("review"), b]).unwrap_err();
    assert!(err.contains("\"fix\"") && err.contains("when"), "{err}");
}

#[test]
fn valid_when_accepted_in_linear_and_dag() {
    let mut b = agent("fix");
    b.when = Some("artifacts.review.verdict not in ('approved', 'ok')".into());
    assert!(validate_pipeline(&[agent("review"), b.clone()]).is_ok());
    b.depends_on = vec!["review".into()];
    assert!(validate_pipeline(&[agent("review"), b]).is_ok());
}

#[test]
fn plan_lines_show_when() {
    let mut b = agent("fix");
    b.when = Some("artifacts.review.bad".into());
    let lines = build_plan_lines(&[agent("review"), b]);
    assert!(lines[1].contains("when: artifacts.review.bad"), "{lines:?}");
    assert!(!lines[0].contains("when:"));
}

// -- fan-out groups ----------------------------------------------------------

fn group(name: &str, nested: Vec<PhaseConfig>) -> PhaseConfig {
    let mut g = PhaseConfig::new(name);
    g.for_each = Some("artifacts.design.tasks".into());
    g.as_var = Some("task".into());
    g.phases = nested;
    g
}

fn script(name: &str) -> PhaseConfig {
    let mut p = PhaseConfig::new(name);
    p.phase_type = PhaseType::Script;
    p.command = Some("true".into());
    p
}

fn retrying(mut p: PhaseConfig, target: &str) -> PhaseConfig {
    p.on_failure = OnFailure::Retry(target.into());
    p
}

#[test]
fn group_valid() {
    let g = group(
        "implement",
        vec![agent("code"), retrying(script("check"), "code")],
    );
    assert!(validate_pipeline(&[agent("design"), g, agent("verify")]).is_ok());
    // Outside phases may loop back to the group itself.
    let mut verify = agent("verify");
    verify.loop_back_to = Some("implement".into());
    let g = group("implement", vec![agent("code")]);
    assert!(validate_pipeline(&[agent("design"), g, verify]).is_ok());
}

#[test]
fn group_errors() {
    let err = |phases: &[PhaseConfig]| validate_pipeline(phases).unwrap_err();

    let mut bad = group("implement", vec![agent("code")]);
    bad.for_each = Some("artifacts.x ==".into());
    assert!(err(&[bad]).contains("invalid for_each"));

    let inner = group("inner", vec![agent("a")]);
    let e = err(&[group("outer", vec![inner])]);
    assert!(
        e.contains("\"outer\"") && e.contains("\"inner\"") && e.contains("not supported"),
        "{e}"
    );

    let e = err(&[group("g", vec![agent("a"), agent("a")])]);
    assert!(e.contains("more than once"), "{e}");

    let e = err(&[group("g", vec![agent("a.b")])]);
    assert!(e.contains("no dots"), "{e}");

    let e = err(&[
        agent("design"),
        group("g", vec![retrying(agent("a"), "design")]),
    ]);
    assert!(
        e.contains("retry \"design\"") && e.contains("same group"),
        "{e}"
    );

    let mut looping = agent("a");
    looping.loop_back_to = Some("design".into());
    let e = err(&[agent("design"), group("g", vec![looping])]);
    assert!(e.contains("loop_back_to \"design\""), "{e}");

    let e = err(&[group("g", vec![agent("a")]), agent("g.x")]);
    assert!(e.contains("clashes"), "{e}");

    // Expanded names aren't pipeline phases: an outside loop can't target them.
    let mut verify = agent("verify");
    verify.loop_back_to = Some("g.1.a".into());
    let e = err(&[group("g", vec![agent("a")]), verify]);
    assert!(e.contains("not found"), "{e}");
}

#[test]
fn group_in_dag_rejected() {
    let mut b = agent("b");
    b.depends_on = vec!["a".into()];
    let e = validate_pipeline(&[agent("a"), b, group("g", vec![agent("x")])]).unwrap_err();
    assert!(e.contains("linear pipeline"), "{e}");
}

#[test]
fn plan_lines_show_group_and_nested() {
    let g = group("implement", vec![agent("code"), script("check")]);
    let lines = build_plan_lines(&[agent("design"), g]);
    assert_eq!(lines.len(), 4);
    assert!(lines[1].contains("implement") && lines[1].contains("for_each"));
    assert!(lines[1].contains("artifacts.design.tasks"));
    assert!(lines[2].contains("code") && lines[2].contains("[agent]"));
    assert!(lines[3].contains("check") && lines[3].contains("[script]"));
}

#[test]
fn estimate_prices_one_item_of_a_group() {
    let g = group("implement", vec![agent("code"), script("check")]);
    let est = flows_core::estimate_pipeline(
        &[agent("design"), g],
        "idea",
        "software",
        Path::new("."),
        &Default::default(),
        &Default::default(),
    );
    let names: Vec<&str> = est.iter().map(|e| e.phase.as_str()).collect();
    assert_eq!(
        names,
        ["design", "implement.<each>.code", "implement.<each>.check"]
    );
}
