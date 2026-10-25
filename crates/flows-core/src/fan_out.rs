//! Fan-out groups: a phase entry with `for_each` runs its nested `phases` once
//! per item of a list produced at run time (typically a task list in an earlier
//! artifact).
//!
//! ```yaml
//! - name: implement
//!   for_each: "artifacts.design.tasks"
//!   as: task
//!   phases:
//!     - name: code              # → implement.<id>.code, once per task
//!     - name: check
//!       type: script
//!       command: "@auto:test"
//!       on_failure: retry:code  # → retries the same task's code phase
//! ```
//!
//! The runner evaluates the expression ([`crate::condition::eval_list`]), turns
//! the result into ordered [`Item`]s ([`items`]) and splices the phases from
//! [`expand`] into the pipeline after the group entry. Expanded phases are
//! ordinary phases: unique names, `vars.<as>` via their [`IncludeOrigin`], and
//! short-name artifact scoping under the `<group>.<id>.` prefix.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::models::{FanOutItem, IncludeOrigin, OnFailure, PhaseConfig, DEFAULT_MAX_ITEMS};

/// One fan-out item, normalized.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    /// Ids of items that must finish first.
    pub depends_on: Vec<String>,
    /// The item as exposed to prompts (`vars.<as>`): the source object with
    /// `id` and `title` filled in.
    pub value: Value,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn scalar_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Normalize the evaluated list and order it by item `depends_on`.
///
/// A string item `s` becomes `{id: <position>, title: s}`; an object's id is
/// its `id` field, else its 1-based position. Ids must be unique and made of
/// letters, digits, `.`, `-`, `_`. More than `max_items` items is an error.
pub fn items(raw: Vec<Value>, max_items: u32) -> Result<Vec<Item>, String> {
    if raw.len() > max_items as usize {
        return Err(format!("{} items exceeds max_items {max_items}", raw.len()));
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut seen = HashSet::new();
    for (i, v) in raw.into_iter().enumerate() {
        let pos = (i + 1).to_string();
        let mut obj = match v {
            Value::String(s) => {
                let mut m = Map::new();
                m.insert("title".into(), Value::String(s));
                m
            }
            Value::Object(m) => m,
            other => {
                return Err(format!(
                    "item {pos} must be an object or a string, got {other}"
                ))
            }
        };
        let id = match obj.get("id") {
            None | Some(Value::Null) => pos.clone(),
            Some(v) => scalar_str(v)
                .ok_or_else(|| format!("item {pos}: `id` must be a string or number"))?,
        };
        if !valid_id(&id) {
            return Err(format!(
                "item {pos}: id {id:?} must be non-empty letters, digits, `.`, `-` or `_`"
            ));
        }
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate item id {id:?}"));
        }
        let title = obj
            .get("title")
            .or_else(|| obj.get("name"))
            .and_then(scalar_str)
            .unwrap_or_else(|| id.clone());
        let depends_on = match obj.get("depends_on") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .map(|d| {
                    scalar_str(d)
                        .ok_or_else(|| format!("item {id:?}: depends_on entries must be ids"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(format!("item {id:?}: depends_on must be a list of ids")),
        };
        obj.insert("id".into(), Value::String(id.clone()));
        obj.insert("title".into(), Value::String(title.clone()));
        out.push(Item {
            id,
            title,
            depends_on,
            value: Value::Object(obj),
        });
    }
    order(out)
}

/// Stable topological order: each step takes the first listed item whose
/// dependencies are all placed.
fn order(items: Vec<Item>) -> Result<Vec<Item>, String> {
    let ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    for it in &items {
        if let Some(d) = it.depends_on.iter().find(|d| !ids.contains(d.as_str())) {
            return Err(format!("item {:?} depends on unknown item {d:?}", it.id));
        }
    }
    let mut placed: HashSet<String> = HashSet::new();
    let mut rest = items;
    let mut out = Vec::with_capacity(rest.len());
    while !rest.is_empty() {
        let Some(i) = rest
            .iter()
            .position(|it| it.depends_on.iter().all(|d| placed.contains(d)))
        else {
            let cycle: Vec<&str> = rest.iter().map(|i| i.id.as_str()).collect();
            return Err(format!("item dependency cycle among {}", cycle.join(", ")));
        };
        let it = rest.remove(i);
        placed.insert(it.id.clone());
        out.push(it);
    }
    Ok(out)
}

/// The cap for `group` (its `max_items`, else the default).
pub fn max_items(group: &PhaseConfig) -> u32 {
    group.max_items.unwrap_or(DEFAULT_MAX_ITEMS)
}

/// Name prefix of `group`'s phases for item `id`: `<group>.<id>.`.
pub fn item_prefix(group: &str, id: &str) -> String {
    format!("{group}.{id}.")
}

/// The pipeline phases for `group` over `items` (already ordered), item by
/// item. `template` is the run's template, used for prompt lookup unless the
/// group itself came from an include.
pub fn expand(group: &PhaseConfig, items: &[Item], template: &str) -> Vec<PhaseConfig> {
    let var = group.item_var();
    let local: HashSet<&str> = group.phases.iter().map(|p| p.name.as_str()).collect();
    let template = group
        .origin
        .as_ref()
        .map(|o| o.template.clone())
        .unwrap_or_else(|| template.to_string());
    let outer_vars = group.vars();
    let count = items.len();
    let mut out = Vec::with_capacity(count * group.phases.len());
    for (i, it) in items.iter().enumerate() {
        let prefix = item_prefix(&group.name, &it.id);
        let rename = |n: &str| {
            if local.contains(n) {
                format!("{prefix}{n}")
            } else {
                n.to_string()
            }
        };
        let mut vars = outer_vars.clone();
        vars.insert(var.to_string(), it.value.clone());
        vars.insert(format!("{var}_index"), Value::from(i + 1));
        vars.insert(format!("{var}_count"), Value::from(count));
        for nested in &group.phases {
            let mut p = nested.clone();
            p.name = format!("{prefix}{}", nested.name);
            p.loop_back_to = p.loop_back_to.as_deref().map(rename);
            if let OnFailure::Retry(target) = &p.on_failure {
                p.on_failure = OnFailure::Retry(rename(target));
            }
            p.untrusted = p.untrusted || group.untrusted;
            p.origin = Some(IncludeOrigin {
                template: template.clone(),
                local_name: nested.name.clone(),
                prefix: prefix.clone(),
                vars: vars.clone(),
                fan_out: Some(FanOutItem {
                    group: group.name.clone(),
                    id: it.id.clone(),
                }),
            });
            out.push(p);
        }
    }
    out
}

/// Ids of `failed` items and every item that (transitively) depends on one.
pub fn dependents(items: &[Item], failed: &str) -> HashSet<String> {
    let mut by_dep: HashMap<&str, Vec<&str>> = HashMap::new();
    for it in items {
        for d in &it.depends_on {
            by_dep.entry(d.as_str()).or_default().push(it.id.as_str());
        }
    }
    let mut out = HashSet::new();
    let mut stack = vec![failed];
    while let Some(id) = stack.pop() {
        for &child in by_dep.get(id).map(Vec::as_slice).unwrap_or_default() {
            if out.insert(child.to_string()) {
                stack.push(child);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn strings_and_objects_normalize() {
        let got = items(
            vec![
                json!("Add parser"),
                json!({"id": "x", "title": "T"}),
                json!({"id": 7}),
            ],
            50,
        )
        .unwrap();
        assert_eq!(ids(&got), ["1", "x", "7"]);
        assert_eq!(got[0].title, "Add parser");
        assert_eq!(got[0].value, json!({"id": "1", "title": "Add parser"}));
        assert_eq!(got[2].title, "7");
    }

    #[test]
    fn bad_items_are_errors() {
        assert!(items(vec![json!(3)], 50)
            .unwrap_err()
            .contains("object or a string"));
        assert!(items(vec![json!({"id": "a b"})], 50)
            .unwrap_err()
            .contains("letters"));
        assert!(items(vec![json!({"id": ""})], 50).is_err());
        assert!(items(vec![json!({"id": "a"}), json!({"id": "a"})], 50)
            .unwrap_err()
            .contains("duplicate"));
        let many = (0..80).map(|i| json!({ "id": i })).collect();
        assert!(items(many, 50).unwrap_err().contains("max_items 50"));
    }

    #[test]
    fn depends_on_reorders_stably() {
        let got = items(
            vec![
                json!({"id": "A", "depends_on": ["B"]}),
                json!({"id": "B"}),
                json!({"id": "C"}),
            ],
            50,
        )
        .unwrap();
        assert_eq!(ids(&got), ["B", "A", "C"]);
    }

    #[test]
    fn cycles_and_unknown_deps_fail() {
        let e = items(
            vec![
                json!({"id": "A", "depends_on": ["B"]}),
                json!({"id": "B", "depends_on": ["A"]}),
            ],
            50,
        )
        .unwrap_err();
        assert!(
            e.contains("cycle") && e.contains('A') && e.contains('B'),
            "{e}"
        );
        let e = items(vec![json!({"id": "A", "depends_on": ["Z"]})], 50).unwrap_err();
        assert!(e.contains("unknown item \"Z\""), "{e}");
    }

    fn group() -> PhaseConfig {
        let mut g = PhaseConfig::new("implement");
        g.for_each = Some("artifacts.design.tasks".into());
        g.as_var = Some("task".into());
        let code = PhaseConfig::new("code");
        let mut check = PhaseConfig::new("check");
        check.on_failure = OnFailure::Retry("code".into());
        check.loop_back_to = Some("code".into());
        g.phases = vec![code, check];
        g
    }

    #[test]
    fn expand_prefixes_rewrites_and_sets_vars() {
        let its = items(vec![json!({"id": "1", "title": "One"}), json!("Two")], 50).unwrap();
        let ps = expand(&group(), &its, "software");
        let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "implement.1.code",
                "implement.1.check",
                "implement.2.code",
                "implement.2.check"
            ]
        );
        assert_eq!(
            ps[3].on_failure,
            OnFailure::Retry("implement.2.code".into())
        );
        assert_eq!(ps[3].loop_back_to.as_deref(), Some("implement.2.code"));
        let o = ps[2].origin.as_ref().unwrap();
        assert_eq!(o.template, "software");
        assert_eq!(o.local_name, "code");
        assert_eq!(o.prefix, "implement.2.");
        assert_eq!(o.vars["task"]["title"], "Two");
        assert_eq!(o.vars["task_index"], 2);
        assert_eq!(o.vars["task_count"], 2);
        assert_eq!(
            ps[2].fan_out(),
            Some(&FanOutItem {
                group: "implement".into(),
                id: "2".into()
            })
        );
    }

    #[test]
    fn dependents_are_transitive() {
        let its = items(
            vec![
                json!({"id": "1"}),
                json!({"id": "2", "depends_on": ["1"]}),
                json!({"id": "3", "depends_on": ["2"]}),
                json!({"id": "4"}),
            ],
            50,
        )
        .unwrap();
        let d = dependents(&its, "1");
        assert_eq!(d, HashSet::from(["2".to_string(), "3".to_string()]));
    }
}
