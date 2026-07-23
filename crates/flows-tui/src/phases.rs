//! Phases-pane model — the left sidebar showing each phase and its status.
//! Port of the Textual `PhasePanel`.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseStatus {
    Pending,
    Running,
    Looping,
    Done,
    Failed,
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
        if let Some(p) = self.phases.iter_mut().find(|(n, _)| n == name) {
            p.1 = status;
        }
    }

    pub fn status_of(&self, name: &str) -> Option<PhaseStatus> {
        self.phases.iter().find(|(n, _)| n == name).map(|(_, s)| *s)
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
