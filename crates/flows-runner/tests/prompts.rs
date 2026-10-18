//! Prompt rendering + loop-condition tests exercising the Python-Jinja idioms
//! the shipped templates actually use (via pycompat).

use flows_runner::prompts::{load_prompt, render_prompt};
use flows_runner::runner::eval_loop_condition;
use serde_json::json;

// -- render_prompt with pycompat idioms --------------------------------------

#[test]
fn renders_dict_get_chain() {
    let ctx = json!({
        "idea": "x",
        "prior_artifacts": {"lint": {"content": {"stdout": "all good", "exit_code": 0}}}
    });
    let tmpl = "{% if prior_artifacts.get('lint') %}LINT: {{ prior_artifacts['lint'].get('content', {}).get('stdout', '') }}{% endif %}";
    let out = render_prompt(tmpl, &ctx).unwrap();
    assert_eq!(out, "LINT: all good");
}

#[test]
fn get_with_default_when_missing() {
    let ctx = json!({"prior_artifacts": {"lint": {}}});
    // content missing → default {} → .get('stdout','') → ''
    let tmpl = "[{{ prior_artifacts['lint'].get('content', {}).get('stdout', '') }}]";
    assert_eq!(render_prompt(tmpl, &ctx).unwrap(), "[]");
}

#[test]
fn missing_key_get_is_falsy() {
    let ctx = json!({"prior_artifacts": {}});
    let tmpl = "{% if prior_artifacts.get('lint') %}yes{% else %}no{% endif %}";
    assert_eq!(render_prompt(tmpl, &ctx).unwrap(), "no");
}

#[test]
fn fallback_prompt_includes_task_and_repo_tree() {
    // A phase with no matching template prompt (e.g. the `blank` template's `run`
    // phase) falls back to the generic prompt — which must still inject the task
    // and the repository file tree so a bare agent sees the repo.
    let repo = tempfile::tempdir().unwrap();
    let phase = flows_core::models::PhaseConfig::new("run");
    let raw = load_prompt(&phase, "blank", repo.path(), None);
    let ctx = json!({ "idea": "count the lines", "repo_tree": "notes.txt\nsrc/lib.rs" });
    let out = render_prompt(&raw, &ctx).expect("fallback prompt must render");
    assert!(out.contains("count the lines"), "task injected");
    assert!(out.contains("notes.txt"), "repo tree injected");
    assert!(out.contains("**run** phase"), "phase name filled");
    assert!(
        !out.contains("Earlier phases"),
        "nothing before the first phase"
    );
}

#[test]
fn fallback_prompt_carries_prior_artifacts_and_pushed_context() {
    // A custom template without prompt files: `implement` must still see the
    // plan, and context features must still reach the model.
    let repo = tempfile::tempdir().unwrap();
    let phase = flows_core::models::PhaseConfig::new("implement");
    let raw = load_prompt(&phase, "e2e", repo.path(), None);
    let ctx = json!({
        "idea": "add L",
        "repo_tree": "cron.go",
        "prior_artifacts": {"plan": {"content": {"steps": ["parse L in day-of-month"]}}},
        "repo_map": "### cron.go\n- func Parse(expr string) (Spec, error)",
        "pushed_context": "# Reference code\nfunc lastDay() {}",
        "tools_enabled": true,
        "tools_list": "- `read_file`",
        "changes": "+func lastDayOfMonth() {}",
    });
    let out = render_prompt(&raw, &ctx).expect("fallback prompt must render");
    assert!(
        out.contains("## plan") && out.contains("parse L in day-of-month"),
        "{out}"
    );
    assert!(out.contains("func Parse(expr string)"), "repo map: {out}");
    assert!(out.contains("func lastDay()"), "pushed context: {out}");
    assert!(out.contains("`read_file`"), "tools: {out}");
    assert!(out.contains("+func lastDayOfMonth() {}"), "changes: {out}");
}

#[test]
fn feature_review_shows_the_changes() {
    let repo = tempfile::tempdir().unwrap();
    let no_overrides = tempfile::tempdir().unwrap();
    let phase = flows_core::models::PhaseConfig::new("review");
    let raw = load_prompt(&phase, "feature", repo.path(), Some(no_overrides.path()));
    let ctx = json!({"idea": "add L", "repo_tree": "cron.go", "prior_artifacts": {},
                     "changes": "+func lastDayOfMonth() {}"});
    let out = render_prompt(&raw, &ctx).unwrap();
    assert!(
        out.contains("Changes under review") && out.contains("+func lastDayOfMonth() {}"),
        "{out}"
    );
    let bare = render_prompt(
        &raw,
        &json!({"idea": "add L", "repo_tree": "", "prior_artifacts": {}}),
    )
    .unwrap();
    assert!(!bare.contains("Changes under review") && bare.contains("read the changed files"));
}

#[test]
fn builtin_feature_prompts_render() {
    // The actual shipped prompts must render (not fall back) with a realistic ctx.
    let repo = tempfile::tempdir().unwrap();
    let mut phase = flows_core::models::PhaseConfig::new("implement");
    let raw = load_prompt(&phase, "feature", repo.path(), None);
    let ctx = json!({
        "idea": "add dark mode",
        "repo_tree": "src/",
        "prior_artifacts": {
            "plan": {"content": {"summary": "do the thing", "steps": ["a", "b"]}}
        }
    });
    let out = render_prompt(&raw, &ctx).expect("feature/implement.md must render");
    // The idea now lives in the user message, not the template; the template must
    // still render prior-artifact context (proves it didn't fall back to raw).
    assert!(out.contains("do the thing"));

    // review.md uses the lint/test .get chains
    phase.name = "review".into();
    let raw = load_prompt(&phase, "feature", repo.path(), None);
    let ctx = json!({
        "idea": "add dark mode",
        "repo_tree": "src/",
        "prior_artifacts": {
            "lint": {"content": {"stdout": "All checks passed!", "stderr": "", "exit_code": 0}},
            "test": {"content": {"stdout": "5 passed", "stderr": "", "exit_code": 0}},
            "implement": {"content": {"summary": "did it"}}
        }
    });
    let out = render_prompt(&raw, &ctx).expect("feature/review.md must render");
    assert!(out.contains("All checks passed!"));
}

// -- loop conditions (the real ones from the templates) ----------------------

#[test]
fn review_verdict_condition() {
    let cond = "content.get('verdict') not in ('approved', 'approved_with_suggestions')";
    assert!(eval_loop_condition(
        cond,
        &json!({"verdict": "needs_fixes"})
    ));
    assert!(!eval_loop_condition(cond, &json!({"verdict": "approved"})));
    assert!(!eval_loop_condition(
        cond,
        &json!({"verdict": "approved_with_suggestions"})
    ));
}

#[test]
fn human_gate_strip_condition() {
    let cond =
        "content.get('answer', '').strip() not in ('', 'ok', 'y', 'yes', 'lgtm', 'approve', 'approved')";
    // real feedback → loop
    assert!(eval_loop_condition(
        cond,
        &json!({"answer": "  make it blue  "})
    ));
    // acceptance words (with whitespace) → don't loop
    assert!(!eval_loop_condition(cond, &json!({"answer": "  ok  "})));
    assert!(!eval_loop_condition(cond, &json!({"answer": ""})));
    assert!(!eval_loop_condition(cond, &json!({"answer": "yes"})));
}

#[test]
fn builtin_prompt_files_includes_nested() {
    let files = flows_runner::builtin_prompt_files();
    assert!(files
        .iter()
        .any(|(p, c)| p == "feature/plan.md" && !c.is_empty()));
    assert!(files.iter().any(|(p, _)| p == "software/spec.md"));
    assert!(files.len() >= 12);
}

#[test]
fn global_prompt_override_is_used() {
    // Simulate `templates sync` + user edit: a custom prompt in flows_dir/prompts.
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    let pdir = flows.path().join("prompts").join("feature");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("plan.md"), "MY CUSTOM PLAN PROMPT").unwrap();

    let phase = flows_core::models::PhaseConfig::new("plan");
    let text = load_prompt(&phase, "feature", repo.path(), Some(flows.path()));
    assert_eq!(text, "MY CUSTOM PLAN PROMPT");
}

#[test]
fn agent_docs_combines_global_and_project() {
    use flows_runner::prompts::load_agent_docs;
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();

    // neither present → None
    assert!(load_agent_docs(repo.path(), Some(flows.path())).is_none());

    std::fs::write(
        flows.path().join("AGENTS.md"),
        "GLOBAL: use make for all tooling.\n",
    )
    .unwrap();
    std::fs::write(repo.path().join("AGENTS.md"), "PROJECT: run `make test`.\n").unwrap();

    let docs = load_agent_docs(repo.path(), Some(flows.path())).unwrap();
    assert!(docs.contains("GLOBAL: use make"));
    assert!(docs.contains("PROJECT: run `make test`"));
    // project comes after global (wins on conflict)
    assert!(docs.find("GLOBAL").unwrap() < docs.find("PROJECT").unwrap());
}
