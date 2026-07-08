//! Port of `flows/usage.py` — parse llm.jsonl into per-phase and total
//! usage + cost.

use std::path::Path;

use serde_json::Value;

use crate::pricing::{calculate_cost, lookup_pricing, tokens_per_second};

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseUsage {
    pub phase: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub total_latency_ms: i64,
    pub rounds: u32,
}

impl PhaseUsage {
    fn new(phase: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            model: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            total_latency_ms: 0,
            rounds: 0,
        }
    }

    pub fn tokens_per_second(&self) -> f64 {
        tokens_per_second(self.output_tokens, self.total_latency_ms)
    }

    pub fn pricing_known(&self) -> bool {
        self.model.is_empty() || lookup_pricing(&self.model).is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageReport {
    pub run_id: String,
    pub idea: String,
    pub phases: Vec<PhaseUsage>,
    /// Input tokens of the most recent request — the current context-window size.
    pub last_input_tokens: u64,
    /// Largest input-token count seen across the run — the peak context size.
    pub peak_input_tokens: u64,
}

impl UsageReport {
    pub fn total_input(&self) -> u64 {
        self.phases.iter().map(|p| p.input_tokens).sum()
    }

    pub fn total_output(&self) -> u64 {
        self.phases.iter().map(|p| p.output_tokens).sum()
    }

    pub fn total_cost(&self) -> f64 {
        self.phases.iter().map(|p| p.cost_usd).sum()
    }

    pub fn total_latency_ms(&self) -> i64 {
        self.phases.iter().map(|p| p.total_latency_ms).sum()
    }

    pub fn overall_tokens_per_second(&self) -> f64 {
        tokens_per_second(self.total_output(), self.total_latency_ms())
    }

    pub fn any_unknown_pricing(&self) -> bool {
        self.phases
            .iter()
            .any(|p| !p.model.is_empty() && !p.pricing_known())
    }
}

/// Index of `phase` in `phases`, inserting a fresh accumulator if absent.
/// A Vec keeps the phase_start insertion order, matching Python's dict.
fn get_or_insert(phases: &mut Vec<PhaseUsage>, phase: &str) -> usize {
    if let Some(i) = phases.iter().position(|p| p.phase == phase) {
        i
    } else {
        phases.push(PhaseUsage::new(phase));
        phases.len() - 1
    }
}

pub fn parse_usage(log_path: &Path, run_id: &str, idea: &str) -> UsageReport {
    let mut phases: Vec<PhaseUsage> = Vec::new();
    let mut last_input_tokens: u64 = 0;
    let mut peak_input_tokens: u64 = 0;

    let text = match std::fs::read_to_string(log_path) {
        Ok(t) => t,
        Err(_) => {
            return UsageReport {
                run_id: run_id.into(),
                idea: idea.into(),
                phases,
                last_input_tokens,
                peak_input_tokens,
            };
        }
    };

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let ev: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event = ev.get("event").and_then(Value::as_str).unwrap_or("");
        let phase = ev.get("phase").and_then(Value::as_str).unwrap_or("");

        match event {
            "phase_start" => {
                get_or_insert(&mut phases, phase);
            }
            "response" => {
                let i = get_or_insert(&mut phases, phase);
                let usage = ev.get("usage");
                let in_tok = usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let out_tok = usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let latency = ev.get("latency_ms").and_then(Value::as_i64).unwrap_or(0);

                // The latest response's input tokens = the live context size.
                if in_tok > 0 {
                    last_input_tokens = in_tok;
                    peak_input_tokens = peak_input_tokens.max(in_tok);
                }
                let pu = &mut phases[i];
                pu.input_tokens += in_tok;
                pu.output_tokens += out_tok;
                pu.total_latency_ms += latency;
                pu.rounds += 1;
                if let Some(m) = ev.get("model").and_then(Value::as_str) {
                    if !m.is_empty() {
                        pu.model = m.to_string();
                    }
                }
                // recompute cost from raw tokens (source of truth)
                pu.cost_usd = calculate_cost(&pu.model, pu.input_tokens, pu.output_tokens);
            }
            _ => {}
        }
    }

    UsageReport {
        run_id: run_id.into(),
        idea: idea.into(),
        phases,
        last_input_tokens,
        peak_input_tokens,
    }
}
