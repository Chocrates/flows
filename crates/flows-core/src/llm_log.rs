//! JSONL logger for LLM interactions, written to `<run_dir>/llm.jsonl`. Port of
//! `flows/llm_log.py`. The format is what [`crate::usage::parse_usage`] reads
//! (per-phase tokens + cost).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::pricing::{calculate_cost, tokens_per_second};

pub struct LlmLogger {
    run_id: String,
    path: PathBuf,
}

impl LlmLogger {
    pub fn new(run_id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self {
            run_id: run_id.into(),
            path,
        }
    }

    fn write(&self, mut event: Map<String, Value>) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        event.entry("ts".to_string()).or_insert(json!(ts));
        event
            .entry("run_id".to_string())
            .or_insert(json!(self.run_id));
        if let Ok(line) = serde_json::to_string(&Value::Object(event)) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    pub fn phase_start(&self, phase: &str) {
        self.write(json_map(json!({"event": "phase_start", "phase": phase})));
    }

    pub fn phase_end(
        &self,
        phase: &str,
        artifact_written: bool,
        total_in: u64,
        total_out: u64,
        model: &str,
    ) {
        let cost = calculate_cost(model, total_in, total_out);
        self.write(json_map(json!({
            "event": "phase_end", "phase": phase, "model": model,
            "artifact_written": artifact_written,
            "total_input_tokens": total_in, "total_output_tokens": total_out,
            "cost_usd": cost,
        })));
    }

    /// Log one model response (the event `parse_usage` sums for cost).
    pub fn response(
        &self,
        phase: &str,
        round: u32,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        latency_ms: i64,
    ) {
        let cost = calculate_cost(model, input_tokens, output_tokens);
        let tps = tokens_per_second(output_tokens, latency_ms);
        self.write(json_map(json!({
            "event": "response", "phase": phase, "round": round, "model": model,
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens},
            "tokens_per_second": (tps * 100.0).round() / 100.0,
            "cost_usd": cost, "latency_ms": latency_ms,
        })));
    }

    pub fn script(
        &self,
        phase: &str,
        command: &str,
        exit_code: i32,
        stdout: &str,
        stderr: &str,
        duration_ms: i64,
    ) {
        let trunc = |s: &str| s.chars().take(2000).collect::<String>();
        self.write(json_map(json!({
            "event": "script", "phase": phase, "command": command, "exit_code": exit_code,
            "stdout": trunc(stdout), "stderr": trunc(stderr), "duration_ms": duration_ms,
        })));
    }

    pub fn error(&self, phase: &str, error_type: &str, message: &str) {
        self.write(json_map(json!({
            "event": "error", "phase": phase, "error": error_type, "message": message,
        })));
    }
}

fn json_map(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// The conventional log path for a run dir.
pub fn log_path(run_dir: &Path) -> PathBuf {
    run_dir.join("llm.jsonl")
}
