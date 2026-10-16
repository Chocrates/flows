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
