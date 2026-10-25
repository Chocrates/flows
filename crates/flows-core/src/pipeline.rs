//! Pipeline validation (port of `_validate_pipeline` / `_build_deps` from
//! `flows/runner.py`). Lives in core so both dry-run and the runner share it.

use std::collections::HashSet;

use crate::models::PhaseConfig;

/// Validate a pipeline; the error string matches the Python `ValueError` text.
pub fn validate_pipeline(phases: &[PhaseConfig]) -> Result<(), String> {
    let names: HashSet<&str> = phases.iter().map(|p| p.name.as_str()).collect();
    let is_dag = phases.iter().any(|p| !p.depends_on.is_empty());

    for p in phases {
        if let Some(cond) = &p.when {
            crate::condition::check_when(cond)
                .map_err(|e| format!("Phase {:?}: invalid when condition {cond:?}: {e}", p.name))?;
        }
        if p.is_group() {
            if is_dag {
                return Err(format!(
                    "Phase {:?}: for_each groups require a linear pipeline (no depends_on)",
                    p.name
                ));
            }
            validate_group(p, phases)?;
        }
    }

    if is_dag {
        for p in phases {
            for dep in &p.depends_on {
                if !names.contains(dep.as_str()) {
                    return Err(format!(
                        "Phase {:?}: depends_on {:?} not found in pipeline",
                        p.name, dep
                    ));
                }
            }
            if p.loop_back_to.is_some() {
                return Err(format!(
                    "Phase {:?}: loop_back_to cannot be used with depends_on. \
                     Use a linear pipeline (no depends_on) for loops.",
                    p.name
                ));
            }
            if matches!(p.on_failure, crate::models::OnFailure::Retry(_)) {
                return Err(format!(
                    "Phase {:?}: retry:X on_failure not supported in DAG mode. \
                     Use 'stop' or 'continue'.",
                    p.name
                ));
            }
        }
    } else {
        for p in phases {
            if let Some(target) = &p.loop_back_to {
                if !names.contains(target.as_str()) {
                    return Err(format!(
                        "Phase {:?}: loop_back_to {:?} not found in pipeline",
                        p.name, target
                    ));
                }
            }
        }
    }
    Ok(())
}

/// A fan-out group's own rules: see `flows_core::fan_out`.
fn validate_group(g: &PhaseConfig, phases: &[PhaseConfig]) -> Result<(), String> {
    let expr = g.for_each.as_deref().unwrap_or_default();
    crate::condition::check_when(expr).map_err(|e| {
        format!(
            "Phase {:?}: invalid for_each expression {expr:?}: {e}",
            g.name
        )
    })?;
    let prefix = format!("{}.", g.name);
    if let Some(p) = phases.iter().find(|p| p.name.starts_with(&prefix)) {
        return Err(format!(
            "Phase {:?}: name clashes with the phases of for_each group {:?}",
            p.name, g.name
        ));
    }
    let mut local = HashSet::new();
    for n in &g.phases {
        if n.name.is_empty() || n.name.contains('.') {
            return Err(format!(
                "Phase {:?} in group {:?}: nested phase names must be non-empty and contain no dots",
                n.name, g.name
            ));
        }
        if !local.insert(n.name.as_str()) {
            return Err(format!(
                "Group {:?}: nested phase {:?} appears more than once",
                g.name, n.name
            ));
        }
    }
    for n in &g.phases {
        if n.is_group() {
            return Err(format!(
                "Group {:?}: nested phase {:?} is itself a for_each group (not supported)",
                g.name, n.name
            ));
        }
        if !n.depends_on.is_empty() {
            return Err(format!(
                "Phase {:?} in group {:?}: depends_on is not supported inside a group",
                n.name, g.name
            ));
        }
        if let Some(cond) = &n.when {
            crate::condition::check_when(cond)
                .map_err(|e| format!("Phase {:?}: invalid when condition {cond:?}: {e}", n.name))?;
        }
        let retry = match &n.on_failure {
            crate::models::OnFailure::Retry(t) => Some(t),
            _ => None,
        };
        for (what, target) in [("loop_back_to", n.loop_back_to.as_ref()), ("retry", retry)] {
            if let Some(t) = target.filter(|t| !local.contains(t.as_str())) {
                return Err(format!(
                    "Phase {:?} in group {:?}: {what} {t:?} must name a phase of the same group",
                    n.name, g.name
                ));
            }
        }
    }
    Ok(())
}

/// Explicit dependency map. Phases without `depends_on` have no deps.
pub fn build_deps(phases: &[PhaseConfig]) -> Vec<(String, Vec<String>)> {
    phases
        .iter()
        .map(|p| (p.name.clone(), p.depends_on.clone()))
        .collect()
}

/// Whether the pipeline runs as a DAG (any phase declares `depends_on`).
pub fn is_dag(phases: &[PhaseConfig]) -> bool {
    phases.iter().any(|p| !p.depends_on.is_empty())
}
