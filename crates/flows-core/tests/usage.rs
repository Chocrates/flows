//! Ports of the behavior in `tests/test_usage.py`.

use std::io::Write;

use flows_core::usage::parse_usage;
use tempfile::NamedTempFile;

fn log(lines: &[&str]) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    f.flush().unwrap();
    f
}

#[test]
fn missing_file_is_empty_report() {
    let r = parse_usage(std::path::Path::new("/no/such/file.jsonl"), "r1", "idea");
    assert_eq!(r.run_id, "r1");
    assert!(r.phases.is_empty());
    assert_eq!(r.total_cost(), 0.0);
}

#[test]
fn accumulates_tokens_and_cost_per_phase() {
    let f = log(&[
        r#"{"event":"phase_start","phase":"plan"}"#,
        r#"{"event":"response","phase":"plan","model":"claude-sonnet-4-6","usage":{"input_tokens":1000000,"output_tokens":0},"latency_ms":500}"#,
        r#"{"event":"response","phase":"plan","model":"claude-sonnet-4-6","usage":{"input_tokens":0,"output_tokens":1000000},"latency_ms":500}"#,
    ]);
    let r = parse_usage(f.path(), "", "");
    assert_eq!(r.phases.len(), 1);
    let p = &r.phases[0];
    assert_eq!(p.input_tokens, 1_000_000);
    assert_eq!(p.output_tokens, 1_000_000);
    assert_eq!(p.rounds, 2);
    assert_eq!(p.total_latency_ms, 1000);
    // 1M in @ $3 + 1M out @ $15 = $18
    assert!((p.cost_usd - 18.0).abs() < 1e-9);
    assert!((r.total_cost() - 18.0).abs() < 1e-9);
}

#[test]
fn preserves_phase_start_order() {
    let f = log(&[
        r#"{"event":"phase_start","phase":"plan"}"#,
        r#"{"event":"phase_start","phase":"implement"}"#,
        r#"{"event":"phase_start","phase":"review"}"#,
        r#"{"event":"response","phase":"review","usage":{"input_tokens":1,"output_tokens":1}}"#,
        r#"{"event":"response","phase":"plan","usage":{"input_tokens":1,"output_tokens":1}}"#,
    ]);
    let r = parse_usage(f.path(), "", "");
    let names: Vec<_> = r.phases.iter().map(|p| p.phase.as_str()).collect();
    assert_eq!(names, vec!["plan", "implement", "review"]);
}

#[test]
fn skips_blank_and_malformed_lines() {
    let f = log(&[
        "",
        "not json",
        r#"{"event":"response","phase":"plan","model":"claude-sonnet-4-6","usage":{"input_tokens":10,"output_tokens":5},"latency_ms":100}"#,
    ]);
    let r = parse_usage(f.path(), "", "");
    assert_eq!(r.phases.len(), 1);
    assert_eq!(r.phases[0].input_tokens, 10);
}

#[test]
fn response_without_prior_phase_start_still_counts() {
    let f = log(&[
        r#"{"event":"response","phase":"solo","model":"gpt-4o","usage":{"input_tokens":100,"output_tokens":50},"latency_ms":200}"#,
    ]);
    let r = parse_usage(f.path(), "", "");
    assert_eq!(r.phases.len(), 1);
    assert_eq!(r.phases[0].phase, "solo");
    assert_eq!(r.total_output(), 50);
}

#[test]
fn unknown_model_pricing_flagged_and_zero_cost() {
    let f = log(&[
        r#"{"event":"response","phase":"x","model":"made-up-model","usage":{"input_tokens":1000,"output_tokens":1000},"latency_ms":100}"#,
    ]);
    let r = parse_usage(f.path(), "", "");
    assert_eq!(r.phases[0].cost_usd, 0.0);
    assert!(!r.phases[0].pricing_known());
    assert!(r.any_unknown_pricing());
}

#[test]
fn empty_model_is_not_unknown_pricing() {
    let f = log(&[r#"{"event":"phase_start","phase":"p"}"#]);
    let r = parse_usage(f.path(), "", "");
    assert!(r.phases[0].pricing_known());
    assert!(!r.any_unknown_pricing());
}

#[test]
fn overall_throughput() {
    let f = log(&[
        r#"{"event":"response","phase":"a","usage":{"input_tokens":0,"output_tokens":100},"latency_ms":1000}"#,
        r#"{"event":"response","phase":"b","usage":{"input_tokens":0,"output_tokens":100},"latency_ms":1000}"#,
    ]);
    let r = parse_usage(f.path(), "", "");
    // 200 output tokens over 2000ms = 100 tok/s
    assert!((r.overall_tokens_per_second() - 100.0).abs() < 1e-9);
}
