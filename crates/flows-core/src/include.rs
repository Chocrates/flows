//! Pipeline composition: `- include: <template>` entries in a phases list are
//! replaced by that template's phases, at load time, so validation and the
//! runner only ever see a flat pipeline.
//!
//! ```yaml
//! phases:
//!   - name: implement
//!   - include: checks            # → lint, test
//!   - include: checks
//!     as: final                  # → final.lint, final.test
//!     with: { target: "make ci" }  # → `vars.target` in prompts/commands/when
//!     when: "artifacts.implement.changed"
//!     depends_on: [implement]    # DAG pipelines only
//! ```
//!
//! Included phases keep their names (a clash is an error; `as:` prefixes them
//! and rewrites references among them). Each carries an [`IncludeOrigin`] so the
//! runner can find its prompt under the template it came from, expose `vars`,
//! and show artifacts of an aliased include under their short names.

use std::collections::{HashMap, HashSet};

use serde_yaml_ng::Value;

use crate::config::{expand_value, parse_phase, ConfigError};
use crate::models::{IncludeOrigin, OnFailure, PhaseConfig};

/// Template name → its YAML text.
pub type Resolver<'a> = dyn Fn(&str) -> Result<String, String> + 'a;

/// Resolver for contexts without template lookup: any include is an error.
pub fn no_includes(name: &str) -> Result<String, String> {
    Err(format!(
        "`include: {name}` is only supported in templates and .flows.yml"
    ))
}

/// Max include nesting.
const MAX_DEPTH: usize = 8;

const INCLUDE_KEYS: &[&str] = &["include", "as", "with", "when", "depends_on"];

fn err(msg: impl Into<String>) -> ConfigError {
    ConfigError::Include(msg.into())
}

/// Expand a raw phases list into a flat pipeline.
pub fn expand(raw: &[Value], resolve: &Resolver<'_>) -> Result<Vec<PhaseConfig>, ConfigError> {
    let phases = expand_level(raw, resolve, &mut Vec::new())?;
    let mut seen = HashSet::new();
    for p in &phases {
        if !seen.insert(p.name.as_str()) {
            let hint = match &p.origin {
                Some(o) => format!(" (from `include: {}`; add `as: <alias>`)", o.template),
                None => String::new(),
            };
            return Err(err(format!(
                "phase name {:?} appears more than once{hint}",
                p.name
            )));
        }
    }
    Ok(phases)
}

/// One include entry after its phases were expanded.
struct Group {
    label: String,
    names: Vec<String>,
}

fn expand_level(
    raw: &[Value],
    resolve: &Resolver<'_>,
    stack: &mut Vec<String>,
) -> Result<Vec<PhaseConfig>, ConfigError> {
    // A level runs as a DAG when any of its own entries declares depends_on.
    let level_dag = raw.iter().any(|v| {
        v.get("depends_on")
            .and_then(Value::as_sequence)
            .is_some_and(|s| !s.is_empty())
    });
    let mut out: Vec<PhaseConfig> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    for entry in raw {
        let Some(template) = entry.get("include") else {
            out.push(parse_phase(entry)?);
            continue;
        };
        let template = template
            .as_str()
            .ok_or_else(|| err("`include:` must name a template"))?
            .to_string();
        let mut group = include_entry(entry, &template, level_dag, resolve, stack)?;
        groups.push(Group {
            label: entry
                .get("as")
                .and_then(Value::as_str)
                .unwrap_or(&template)
                .to_string(),
            names: group.iter().map(|p| p.name.clone()).collect(),
        });
        out.append(&mut group);
    }

    // `depends_on: [<include label>]` → every phase of that include. A phase
    // name wins over a label.
    if level_dag && !groups.is_empty() {
        let names: HashSet<String> = out.iter().map(|p| p.name.clone()).collect();
        let labels: HashMap<&str, &[String]> = groups
            .iter()
            .map(|g| (g.label.as_str(), g.names.as_slice()))
            .collect();
        for p in &mut out {
            let mut deps = Vec::new();
            for d in std::mem::take(&mut p.depends_on) {
                match labels.get(d.as_str()) {
                    Some(members) if !names.contains(&d) => {
                        deps.extend(members.iter().filter(|m| **m != p.name).cloned())
                    }
                    _ => deps.push(d),
                }
            }
            deps.dedup();
            p.depends_on = deps;
        }
    }
    Ok(out)
}

/// Load, expand, alias and wire one `include:` entry.
fn include_entry(
    entry: &Value,
    template: &str,
    level_dag: bool,
    resolve: &Resolver<'_>,
    stack: &mut Vec<String>,
) -> Result<Vec<PhaseConfig>, ConfigError> {
    if let Some(map) = entry.as_mapping() {
        for k in map.keys() {
            let k = k.as_str().unwrap_or_default();
            if !INCLUDE_KEYS.contains(&k) {
                return Err(err(format!(
                    "`include: {template}`: unknown key {k:?} (allowed: {})",
                    INCLUDE_KEYS.join(", ")
                )));
            }
        }
    }
    if stack.iter().any(|t| t == template) {
        let mut chain = stack.clone();
        chain.push(template.to_string());
        return Err(err(format!("include cycle: {}", chain.join(" → "))));
    }
    if stack.len() >= MAX_DEPTH {
        return Err(err(format!(
            "`include: {template}`: includes nested more than {MAX_DEPTH} deep"
        )));
    }

    let alias = match entry.get("as") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let a = v.as_str().unwrap_or_default().trim();
            if a.is_empty() || a.contains('.') {
                return Err(err(format!(
                    "`include: {template}`: `as:` must be a non-empty name without dots"
                )));
            }
            Some(a.to_string())
        }
    };
    let with = match entry.get("with") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(v @ Value::Mapping(_)) => match serde_json::to_value(v) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => {
                return Err(err(format!(
                    "`include: {template}`: `with:` must be a map with string keys"
                )))
            }
        },
        Some(_) => return Err(err(format!("`include: {template}`: `with:` must be a map"))),
    };
    let when = entry
        .get("when")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_string);
    let entry_deps: Vec<String> = entry
        .get("depends_on")
        .and_then(Value::as_sequence)
        .map(|s| {
            s.iter()
                .filter_map(|d| d.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let text = resolve(template).map_err(|e| err(format!("`include: {template}`: {e}")))?;
    let mut root: Value = serde_yaml_ng::from_str(&text).map_err(|source| ConfigError::Yaml {
        path: format!("<template {template}>"),
        source,
    })?;
    expand_value(&mut root);
    let inner_raw = match root.get("phases") {
        Some(Value::Sequence(seq)) => seq.clone(),
        _ => Vec::new(),
    };
    stack.push(template.to_string());
    let mut phases = expand_level(&inner_raw, resolve, stack)?;
    stack.pop();
    if phases.is_empty() {
        return Err(err(format!(
            "`include: {template}`: template has no phases"
        )));
    }

    // Origin: phases defined directly by this template get one now; phases from
    // a nested include keep theirs, with this include's `with` as defaults.
    for p in &mut phases {
        match &mut p.origin {
            None => {
                p.origin = Some(IncludeOrigin {
                    template: template.to_string(),
                    local_name: p.name.clone(),
                    prefix: String::new(),
                    vars: with.clone(),
                    fan_out: None,
                })
            }
            Some(o) => {
                let mut vars = with.clone();
                vars.extend(std::mem::take(&mut o.vars));
                o.vars = vars;
            }
        }
    }

    if let Some(alias) = &alias {
        let prefix = format!("{alias}.");
        let local: HashSet<String> = phases.iter().map(|p| p.name.clone()).collect();
        let rename = |n: &str| {
            if local.contains(n) {
                format!("{prefix}{n}")
            } else {
                n.to_string()
            }
        };
        for p in &mut phases {
            p.name = format!("{prefix}{}", p.name);
            p.depends_on = p.depends_on.iter().map(|d| rename(d)).collect();
            p.loop_back_to = p.loop_back_to.as_deref().map(rename);
            if let OnFailure::Retry(target) = &p.on_failure {
                p.on_failure = OnFailure::Retry(rename(target));
            }
            if let Some(o) = &mut p.origin {
                o.prefix = format!("{prefix}{}", o.prefix);
            }
        }
    }

    if let Some(outer) = &when {
        for p in &mut phases {
            p.when = Some(match p.when.take() {
                Some(inner) => format!("({outer}) and ({inner})"),
                None => outer.clone(),
            });
        }
    }

    let inner_dag = phases.iter().any(|p| !p.depends_on.is_empty());
    if inner_dag && !level_dag {
        return Err(err(format!(
            "`include: {template}`: the template uses depends_on, so the including \
             pipeline must use depends_on too (give the include or its neighbours \
             depends_on)"
        )));
    }
    if level_dag {
        if inner_dag {
            // Roots of the included DAG wait for the include's own deps.
            for p in phases.iter_mut().filter(|p| p.depends_on.is_empty()) {
                p.depends_on = entry_deps.clone();
            }
        } else {
            // A linear template keeps its order: chain it.
            let mut prev: Option<String> = None;
            for p in &mut phases {
                p.depends_on = match &prev {
                    Some(n) => vec![n.clone()],
                    None => entry_deps.clone(),
                };
                prev = Some(p.name.clone());
            }
        }
    }
    Ok(phases)
}

/// The artifacts map as seen by `phase`: for a phase from an aliased include,
/// every artifact under that alias prefix is also visible by its short name
/// (overriding an unrelated phase of the same short name).
pub fn scoped_artifacts(
    phase: &PhaseConfig,
    artifacts: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let Some(prefix) = phase
        .origin
        .as_ref()
        .map(|o| o.prefix.as_str())
        .filter(|p| !p.is_empty())
    else {
        return artifacts.clone();
    };
    let mut out = artifacts.clone();
    for (k, v) in artifacts {
        if let Some(short) = k.strip_prefix(prefix) {
            out.insert(short.to_string(), v.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_phases_from_yaml_with;

    fn templates(
        defs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Result<String, String> {
        move |name: &str| {
            defs.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, t)| t.to_string())
                .ok_or_else(|| format!("unknown template {name:?}"))
        }
    }

    const CHECKS: &str = "phases:\n  - name: lint\n    type: script\n    command: \"{{ vars.lint_cmd }}\"\n  - name: test\n    type: script\n    command: make test\n    on_failure: retry:lint\n";

    fn names(ps: &[PhaseConfig]) -> Vec<&str> {
        ps.iter().map(|p| p.name.as_str()).collect()
    }

    fn load(
        yaml: &str,
        defs: &'static [(&'static str, &'static str)],
    ) -> Result<Vec<PhaseConfig>, String> {
        parse_phases_from_yaml_with(yaml, &templates(defs)).map_err(|e| e.to_string())
    }

    #[test]
    fn splices_in_place_with_origin() {
        let ps = load(
            "phases:\n  - name: plan\n  - include: checks\n  - name: ship\n",
            &[("checks", CHECKS)],
        )
        .unwrap();
        assert_eq!(names(&ps), ["plan", "lint", "test", "ship"]);
        let o = ps[1].origin.as_ref().unwrap();
        assert_eq!(
            (o.template.as_str(), o.local_name.as_str()),
            ("checks", "lint")
        );
        assert!(ps[0].origin.is_none());
        assert!(ps[1].depends_on.is_empty(), "linear stays linear");
    }

    #[test]
    fn alias_prefixes_and_rewrites_references() {
        let ps = load(
            "phases:\n  - include: checks\n  - include: checks\n    as: final\n    with: {lint_cmd: make lint}\n",
            &[("checks", CHECKS)],
        )
        .unwrap();
        assert_eq!(names(&ps), ["lint", "test", "final.lint", "final.test"]);
        assert_eq!(ps[3].on_failure, OnFailure::Retry("final.lint".into()));
        assert_eq!(ps[1].on_failure, OnFailure::Retry("lint".into()));
        let o = ps[2].origin.as_ref().unwrap();
        assert_eq!(o.prefix, "final.");
        assert_eq!(o.local_name, "lint");
        assert_eq!(o.vars["lint_cmd"], "make lint");
        assert!(ps[0].vars().is_empty());
    }

    #[test]
    fn clash_without_alias_is_an_error() {
        let e = load(
            "phases:\n  - name: lint\n  - include: checks\n",
            &[("checks", CHECKS)],
        )
        .unwrap_err();
        assert!(e.contains("\"lint\"") && e.contains("as:"), "{e}");
        let e = load(
            "phases:\n  - include: checks\n  - include: checks\n",
            &[("checks", CHECKS)],
        )
        .unwrap_err();
        assert!(e.contains("more than once"), "{e}");
    }

    #[test]
    fn nesting_merges_vars_and_prefixes() {
        const OUTER: &str = "phases:\n  - name: review\n  - include: checks\n    as: inner\n    with: {lint_cmd: inner-lint, x: 1}\n";
        let ps = load(
            "phases:\n  - include: outer\n    as: top\n    with: {lint_cmd: outer-lint, y: 2}\n",
            &[("checks", CHECKS), ("outer", OUTER)],
        )
        .unwrap();
        assert_eq!(
            names(&ps),
            ["top.review", "top.inner.lint", "top.inner.test"]
        );
        let o = ps[1].origin.as_ref().unwrap();
        assert_eq!(o.template, "checks");
        assert_eq!(o.prefix, "top.inner.");
        assert_eq!(o.vars["lint_cmd"], "inner-lint", "inner with wins");
        assert_eq!(o.vars["y"], 2, "outer with is inherited");
        assert_eq!(ps[2].on_failure, OnFailure::Retry("top.inner.lint".into()));
        assert_eq!(ps[0].origin.as_ref().unwrap().template, "outer");
    }

    #[test]
    fn cycles_depth_and_bad_entries_fail() {
        let e = load(
            "phases:\n  - include: a\n",
            &[
                ("a", "phases:\n  - include: b\n"),
                ("b", "phases:\n  - include: a\n"),
            ],
        )
        .unwrap_err();
        assert!(e.contains("cycle: a → b → a"), "{e}");
        let e = load("phases:\n  - include: nope\n", &[]).unwrap_err();
        assert!(e.contains("unknown template"), "{e}");
        let e = load(
            "phases:\n  - include: checks\n    model: x\n",
            &[("checks", CHECKS)],
        )
        .unwrap_err();
        assert!(e.contains("unknown key \"model\""), "{e}");
        let e = parse_phases_from_yaml_with("phases:\n  - include: checks\n", &no_includes)
            .unwrap_err()
            .to_string();
        assert!(e.contains("only supported"), "{e}");
    }

    #[test]
    fn include_when_is_combined() {
        const T: &str = "phases:\n  - name: a\n  - name: b\n    when: \"artifacts.a.ok\"\n";
        let ps = load(
            "phases:\n  - name: gate\n  - include: t\n    when: \"artifacts.gate.go\"\n",
            &[("t", T)],
        )
        .unwrap();
        assert_eq!(ps[1].when.as_deref(), Some("artifacts.gate.go"));
        assert_eq!(
            ps[2].when.as_deref(),
            Some("(artifacts.gate.go) and (artifacts.a.ok)")
        );
    }

    #[test]
    fn dag_parent_chains_linear_include_and_expands_labels() {
        let ps = load(
            "phases:\n  - name: build\n  - include: checks\n    depends_on: [build]\n  - name: ship\n    depends_on: [checks]\n",
            &[("checks", CHECKS)],
        )
        .unwrap();
        let deps: HashMap<&str, Vec<&str>> = ps
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.depends_on.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(deps["lint"], ["build"]);
        assert_eq!(deps["test"], ["lint"]);
        assert_eq!(deps["ship"], ["lint", "test"]);
    }

    #[test]
    fn dag_include_roots_get_entry_deps() {
        const FAN: &str =
            "phases:\n  - name: a\n  - name: b\n  - name: c\n    depends_on: [a, b]\n";
        let ps = load(
            "phases:\n  - name: start\n  - include: fan\n    as: f\n    depends_on: [start]\n",
            &[("fan", FAN)],
        )
        .unwrap();
        assert_eq!(names(&ps), ["start", "f.a", "f.b", "f.c"]);
        assert_eq!(ps[1].depends_on, ["start"]);
        assert_eq!(ps[2].depends_on, ["start"]);
        assert_eq!(ps[3].depends_on, ["f.a", "f.b"]);
    }

    #[test]
    fn dag_template_in_linear_pipeline_is_rejected() {
        const FAN: &str = "phases:\n  - name: a\n  - name: c\n    depends_on: [a]\n";
        let e = load("phases:\n  - name: x\n  - include: fan\n", &[("fan", FAN)]).unwrap_err();
        assert!(e.contains("must use depends_on"), "{e}");
    }

    #[test]
    fn scoped_view_exposes_short_names() {
        let ps = load(
            "phases:\n  - include: checks\n    as: final\n",
            &[("checks", CHECKS)],
        )
        .unwrap();
        let mut arts = serde_json::Map::new();
        arts.insert("lint".into(), serde_json::json!("top-level"));
        arts.insert("final.lint".into(), serde_json::json!("scoped"));
        let view = scoped_artifacts(&ps[1], &arts);
        assert_eq!(view["lint"], "scoped");
        assert_eq!(view["final.lint"], "scoped");
        let plain = scoped_artifacts(&PhaseConfig::new("x"), &arts);
        assert_eq!(plain["lint"], "top-level");
    }
}
