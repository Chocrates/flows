//! Pipeline expressions: phase `when:` conditions and `loop_condition`.
//!
//! Both are MiniJinja boolean expressions with Python-isms accepted (`pycompat`
//! resolves `.get()` / `.strip()`; a tuple literal after `in` becomes a list).
//! They differ in context and in error handling:
//!
//! * `loop_condition` sees the phase's own `content`; any error counts as false.
//! * `when` sees `artifacts.<phase>` (each recorded phase's content) and `idea`;
//!   it is syntax-checked at validation and an evaluation error is returned, so
//!   the runner can fail the run instead of guessing.

use std::sync::OnceLock;

use minijinja::{context, Environment, UndefinedBehavior};
use regex::Regex;
use serde_json::{Map, Value};

/// Python tuple literals `(a, b)` aren't MiniJinja syntax; the loop conditions
/// use `... in ('approved', 'approved_with_suggestions')`. Rewrite only the
/// tuple that follows `in` into a list literal — NOT call-args like
/// `.get('answer', '')`, which also look like `(x, y)`.
pub fn tuples_to_lists(expr: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(\bin\s*)\(([^()]*)\)").unwrap());
    re.replace_all(expr, "${1}[$2]").into_owned()
}

fn env() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    env
}

/// Evaluate a loop condition (a boolean expression over `content`). Empty or
/// erroring conditions are false.
pub fn eval_loop_condition(condition: &str, content: &Value) -> bool {
    if condition.trim().is_empty() {
        return false;
    }
    // Evaluate as an expression and test its truth, rather than rendering it
    // and comparing text: minijinja 2.24 renders booleans as `True`, which
    // silently turned every loop condition false.
    let expr = tuples_to_lists(condition);
    let env = env();
    env.compile_expression(&expr)
        .and_then(|c| c.eval(context! { content => content }))
        .map(|v| v.is_true())
        .unwrap_or(false)
}

fn when_env() -> Environment<'static> {
    let mut env = Environment::new();
    // `artifacts.missing.field` — and `artifacts.missing.get('field')` — is
    // undefined (falsy), not an error.
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.set_unknown_method_callback(|state, value, method, args| {
        if value.is_undefined() {
            return Ok(minijinja::Value::UNDEFINED);
        }
        minijinja_contrib::pycompat::unknown_method_callback(state, value, method, args)
    });
    env
}

/// Check that a `when` expression parses. `Err` carries the parser message.
pub fn check_when(expr: &str) -> Result<(), String> {
    let src = tuples_to_lists(expr);
    when_env()
        .compile_expression(&src)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Evaluate a `when` expression. `artifacts` maps phase name → recorded
/// artifact (the full envelope; only `content` is exposed to the expression).
pub fn eval_when(expr: &str, artifacts: &Map<String, Value>, idea: &str) -> Result<bool, String> {
    eval_when_with_vars(expr, artifacts, idea, &Map::new())
}

/// [`eval_when`] with an include's `with:` values exposed as `vars`.
pub fn eval_when_with_vars(
    expr: &str,
    artifacts: &Map<String, Value>,
    idea: &str,
    vars: &Map<String, Value>,
) -> Result<bool, String> {
    let src = tuples_to_lists(expr);
    let env = when_env();
    let compiled = env.compile_expression(&src).map_err(|e| e.to_string())?;
    let contents: Map<String, Value> = artifacts
        .iter()
        .map(|(k, v)| (k.clone(), v.get("content").cloned().unwrap_or(Value::Null)))
        .collect();
    compiled
        .eval(context! { artifacts => contents, idea => idea, vars => vars })
        .map(|v| v.is_true())
        .map_err(|e| e.to_string())
}

/// Evaluate a fan-out group's `for_each` expression to a list, in the same
/// sandbox and context as `when` (`artifacts`, `idea`, `vars`). A non-list
/// result (including undefined) is an error.
pub fn eval_list(
    expr: &str,
    artifacts: &Map<String, Value>,
    idea: &str,
    vars: &Map<String, Value>,
) -> Result<Vec<Value>, String> {
    let src = tuples_to_lists(expr);
    let env = when_env();
    let compiled = env.compile_expression(&src).map_err(|e| e.to_string())?;
    let contents: Map<String, Value> = artifacts
        .iter()
        .map(|(k, v)| (k.clone(), v.get("content").cloned().unwrap_or(Value::Null)))
        .collect();
    let out = compiled
        .eval(context! { artifacts => contents, idea => idea, vars => vars })
        .map_err(|e| e.to_string())?;
    if out.is_undefined() {
        return Err("expression is undefined (no such artifact or field)".into());
    }
    match serde_json::to_value(&out).map_err(|e| e.to_string())? {
        Value::Array(items) => Ok(items),
        other => Err(format!("expected a list, got {}", json_kind(&other))),
    }
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn arts(v: Value) -> Map<String, Value> {
        v.as_object()
            .unwrap()
            .iter()
            .map(|(k, c)| (k.clone(), json!({ "phase": k, "content": c })))
            .collect()
    }

    #[test]
    fn loop_condition_is_evaluated_not_rendered() {
        // The shipped feature/e2e review condition, both ways.
        let cond = "content.get('verdict') not in ('approved', 'approved_with_suggestions')";
        assert!(eval_loop_condition(
            cond,
            &json!({"verdict": "needs_fixes"})
        ));
        assert!(!eval_loop_condition(cond, &json!({"verdict": "approved"})));
        assert!(eval_loop_condition("content.n > 2", &json!({"n": 3})));
        assert!(!eval_loop_condition("content.n > 2", &json!({"n": 1})));
    }

    #[test]
    fn when_reads_vars() {
        let mut vars = Map::new();
        vars.insert("strict".into(), json!(true));
        assert_eq!(
            eval_when_with_vars("vars.strict", &Map::new(), "", &vars),
            Ok(true)
        );
        assert_eq!(eval_when("vars.strict", &Map::new(), ""), Ok(false));
    }

    #[test]
    fn when_reads_artifact_content() {
        let a = arts(json!({"review": {"verdict": "rejected"}}));
        let cond = "artifacts.review.verdict != 'approved'";
        assert_eq!(eval_when(cond, &a, ""), Ok(true));
        let a = arts(json!({"review": {"verdict": "approved"}}));
        assert_eq!(eval_when(cond, &a, ""), Ok(false));
    }

    #[test]
    fn when_missing_phase_or_field_is_falsy() {
        let a = arts(json!({"review": {}}));
        assert_eq!(eval_when("artifacts.missing.flag", &a, ""), Ok(false));
        assert_eq!(eval_when("artifacts.review.flag", &a, ""), Ok(false));
        assert_eq!(eval_when("not artifacts.missing.flag", &a, ""), Ok(true));
        assert_eq!(
            eval_when("artifacts.missing.get('flag')", &a, ""),
            Ok(false)
        );
    }

    #[test]
    fn when_accepts_pythonisms_and_idea() {
        let a = arts(json!({"plan": {"kind": " schema "}}));
        assert_eq!(
            eval_when(
                "artifacts.plan.get('kind', '').strip() in ('schema', 'data')",
                &a,
                ""
            ),
            Ok(true)
        );
        assert_eq!(
            eval_when("'migrate' in idea", &Map::new(), "migrate users"),
            Ok(true)
        );
    }

    #[test]
    fn when_sees_skipped_marker() {
        let a = arts(json!({"lint": {"skipped": true, "reason": "x"}}));
        assert_eq!(eval_when("artifacts.lint.skipped", &a, ""), Ok(true));
    }

    #[test]
    fn when_syntax_checked() {
        assert!(check_when("artifacts.a.x == 'y'").is_ok());
        assert!(check_when("artifacts.a.x ==").is_err());
        assert!(eval_when("artifacts.a.x ==", &Map::new(), "").is_err());
    }

    #[test]
    fn when_runtime_error_is_returned() {
        assert!(eval_when("no_such_function()", &Map::new(), "").is_err());
    }

    #[test]
    fn eval_list_returns_items() {
        let a = arts(json!({"design": {"tasks": [{"id": "1"}, {"id": "2"}]}}));
        let got = eval_list("artifacts.design.tasks", &a, "", &Map::new()).unwrap();
        assert_eq!(got, vec![json!({"id": "1"}), json!({"id": "2"})]);
        // Expressions, not just paths.
        let got = eval_list("artifacts.design.tasks[1:]", &a, "", &Map::new()).unwrap();
        assert_eq!(got, vec![json!({"id": "2"})]);
    }

    #[test]
    fn eval_list_rejects_non_lists() {
        let a = arts(json!({"design": {"title": "x"}}));
        let e = eval_list("artifacts.design.title", &a, "", &Map::new()).unwrap_err();
        assert!(e.contains("a string"), "{e}");
        let e = eval_list("artifacts.design.tasks", &a, "", &Map::new()).unwrap_err();
        assert!(e.contains("undefined"), "{e}");
        assert!(eval_list("artifacts.design.tasks ==", &a, "", &Map::new()).is_err());
    }
}
