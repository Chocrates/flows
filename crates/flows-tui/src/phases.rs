//! Phases-pane model — the left sidebar showing each phase and its status.
//! Port of the Textual `PhasePanel`.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseStatus {
    Pending,
    Running,
    Looping,
    Done,
    Failed,
    /// `when` condition was false; the phase did not run.
    Skipped,
}

impl PhaseStatus {
    /// Status glyph, matching the Textual icons.
    pub fn glyph(&self) -> char {
        match self {
            PhaseStatus::Pending => '○',
            PhaseStatus::Running => '▶',
            PhaseStatus::Looping => '↻',
            PhaseStatus::Done => '✓',
            PhaseStatus::Failed => '✗',
            PhaseStatus::Skipped => '⊘',
        }
    }
}

/// A loop-back edge: `from` re-runs `to` up to `max` times.
#[derive(Debug, Clone)]
pub struct LoopEdge {
    pub from: String,
    pub to: String,
    pub max: u32,
}

#[derive(Default)]
pub struct PhasePanel {
    phases: Vec<(String, PhaseStatus)>,
    loop_iters: HashMap<String, u32>,
    loops: Vec<LoopEdge>,
    /// Fan-out item rows (`<group>.<id>`): an expanded phase
    /// `<group>.<id>.<phase>` reports its status on its item's row.
    items: HashSet<String>,
    /// Display labels for item rows.
    labels: HashMap<String, String>,
}

impl PhasePanel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn setup(&mut self, names: &[String]) {
        self.phases = names
            .iter()
            .map(|n| (n.clone(), PhaseStatus::Pending))
            .collect();
        self.loop_iters.clear();
    }

    pub fn set_status(&mut self, name: &str, status: PhaseStatus) {
        let exact = self.phases.iter().position(|(n, _)| n == name);
        let Some(row) = exact.or_else(|| self.item_row(name)) else {
            return;
        };
        // One skipped phase of an item doesn't make the whole item skipped.
        if exact.is_none()
            && status == PhaseStatus::Skipped
            && self.phases[row].1 != PhaseStatus::Pending
        {
            return;
        }
        self.phases[row].1 = status;
    }

    pub fn status_of(&self, name: &str) -> Option<PhaseStatus> {
        self.phases
            .iter()
            .position(|(n, _)| n == name)
            .or_else(|| self.item_row(name))
            .map(|i| self.phases[i].1)
    }

    /// The item row an expanded phase name belongs to.
    fn item_row(&self, name: &str) -> Option<usize> {
        self.phases.iter().position(|(n, _)| {
            self.items.contains(n)
                && name
                    .strip_prefix(n.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    }

    /// Show a fan-out group's items as rows under the group (replacing rows
    /// from an earlier expansion).
    pub fn set_items(&mut self, group: &str, items: &[(String, String)]) {
        let prefix = format!("{group}.");
        self.phases
            .retain(|(n, _)| !(n.starts_with(&prefix) && self.items.contains(n)));
        self.items.retain(|n| !n.starts_with(&prefix));
        let Some(at) = self.row_of(group) else {
            return;
        };
        let rows = items.iter().map(|(id, title)| {
            let name = format!("{prefix}{id}");
            let label = if title == id {
                format!("  · {id}")
            } else {
                format!("  · {id} {title}")
            };
            self.labels.insert(name.clone(), label);
            self.items.insert(name.clone());
            (name, PhaseStatus::Pending)
        });
        let rows: Vec<_> = rows.collect();
        self.phases.splice(at + 1..at + 1, rows);
    }

    pub fn set_loop_iteration(&mut self, name: &str, iter: u32) {
        self.loop_iters.insert(name.to_string(), iter);
        self.set_status(name, PhaseStatus::Looping);
    }

    /// Reset the phases in the loop range `[to..=from]` back to Pending so a
    /// loop-back doesn't leave stale `✓` marks on phases that are about to re-run
    /// (e.g. rejecting a design must un-check `design` as it goes back). The caller
    /// then marks the source phase as Looping. Order-agnostic on `to`/`from`.
    pub fn restart_range(&mut self, to: &str, from: &str) {
        let (Some(a), Some(b)) = (self.row_of(to), self.row_of(from)) else {
            return;
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        for (_, status) in self.phases[lo..=hi].iter_mut() {
            *status = PhaseStatus::Pending;
        }
    }

    /// Register the loop-back edges (persistent — shown even before looping).
    pub fn set_loops(&mut self, edges: Vec<LoopEdge>) {
        self.loops = edges;
    }

    fn row_of(&self, name: &str) -> Option<usize> {
        self.phases.iter().position(|(n, _)| n == name)
    }

    /// Loop brackets to draw in the gutter: (top_row, bottom_row, active).
    /// The target sits above the source, so the bracket spans target→source.
    pub fn loop_spans(&self) -> Vec<(usize, usize, bool)> {
        self.loops
            .iter()
            .filter_map(|e| {
                let from = self.row_of(&e.from)?;
                let to = self.row_of(&e.to)?;
                let (top, bottom) = if to <= from { (to, from) } else { (from, to) };
                let active = self.status_of(&e.from) == Some(PhaseStatus::Looping)
                    || self.loop_iters.get(&e.from).copied().unwrap_or(0) > 0;
                Some((top, bottom, active))
            })
            .collect()
    }

    /// Rows to render: (glyph, label, status).
    pub fn rows(&self) -> Vec<(char, String, PhaseStatus)> {
        self.phases
            .iter()
            .map(|(name, status)| {
                // A loop source persistently shows its iteration budget.
                let label = if let Some(edge) = self.loops.iter().find(|e| &e.from == name) {
                    let it = self.loop_iters.get(name).copied().unwrap_or(0);
                    format!("{name} ↺{it}/{}", edge.max)
                } else if let Some(label) = self.labels.get(name) {
                    label.clone()
                } else {
                    name.clone()
                };
                (status.glyph(), label, *status)
            })
            .collect()
    }

    /// True if any phase declares a loop-back (so render can reserve a gutter).
    pub fn has_loops(&self) -> bool {
        !self.loops.is_empty()
    }

    pub fn names(&self) -> Vec<String> {
        self.phases.iter().map(|(n, _)| n.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn items(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn items_appear_under_their_group_and_track_expanded_phases() {
        let mut p = PhasePanel::new();
        p.setup(&names(&["design", "implement", "verify"]));
        p.set_items("implement", &items(&[("1", "Parser"), ("2", "2")]));
        let rows = p.rows();
        let labels: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        assert_eq!(
            labels,
            ["design", "implement", "  · 1 Parser", "  · 2", "verify"]
        );

        p.set_status("implement.1.code", PhaseStatus::Running);
        assert_eq!(p.status_of("implement.1"), Some(PhaseStatus::Running));
        p.set_status("implement.1.check", PhaseStatus::Done);
        // A skipped later phase leaves a run item alone…
        p.set_status("implement.1.extra", PhaseStatus::Skipped);
        assert_eq!(p.status_of("implement.1"), Some(PhaseStatus::Done));
        // …but marks a pending one (e.g. dependents of a failed item).
        p.set_status("implement.2.code", PhaseStatus::Skipped);
        assert_eq!(p.status_of("implement.2"), Some(PhaseStatus::Skipped));
    }

    #[test]
    fn re_expansion_replaces_item_rows() {
        let mut p = PhasePanel::new();
        p.setup(&names(&["implement", "verify"]));
        p.set_items("implement", &items(&[("1", "a"), ("2", "b")]));
        p.set_items("implement", &items(&[("x", "c")]));
        assert_eq!(p.names(), ["implement", "implement.x", "verify"]);
    }
}
