//! Persistent eval results store: an append-only JSONL log of trial records plus
//! per-trial saved output for manual review.
//!
//! Layout under `~/.flows/eval/`:
//! - `results.jsonl` — one [`TrialRecord`] per line, append-only. A later manual
//!   grade appends a *new* full record with the same `trial` id; readers fold by
//!   id taking the most recent, so grades overlay metrics without rewriting.
//! - `trials/<id>/` — `diff.patch` (the run's produced diff) and `meta.json`
//!   (task + rubric) so `eval review` can show the output offline.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One trial's recorded outcome. Serialized as a JSONL line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialRecord {
    pub trial: String,
    /// Unix epoch seconds when written.
    pub ts: u64,
    pub model: String,
    pub config: String,
    pub fixture: String,
    pub suite: String,
    pub size: String,
    /// flow | phase | turn — the pipeline scope this trial exercised. Report
    /// groups by it so granularities are never compared against each other.
    #[serde(default)]
    pub granularity: String,
    /// Shell-verifier outcome (coder suite). `None` when there is no verifier.
    pub verify_pass: Option<bool>,
    /// Human grade (reasoner suite, or an optional coder override). `None` = ungraded.
    pub manual_pass: Option<bool>,
    pub manual_score: Option<u8>,
    pub note: Option<String>,
    pub in_tok: u64,
    pub out_tok: u64,
    pub peak_ctx: u64,
    pub cost: f64,
    pub rounds: u32,
    pub secs: f64,
}

/// `~/.flows/eval`.
pub fn store_dir(flows_dir: &Path) -> PathBuf {
    flows_dir.join("eval")
}

fn results_path(flows_dir: &Path) -> PathBuf {
    store_dir(flows_dir).join("results.jsonl")
}

/// `~/.flows/eval/trials/<id>`.
pub fn trial_dir(flows_dir: &Path, trial_id: &str) -> PathBuf {
    store_dir(flows_dir).join("trials").join(trial_id)
}

/// Append one record as a JSONL line (creating the store dir if needed).
pub fn append(flows_dir: &Path, rec: &TrialRecord) -> Result<(), String> {
    let dir = store_dir(flows_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let line = serde_json::to_string(rec).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(results_path(flows_dir))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// Task / rubric saved alongside a trial's diff for `review`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialMeta {
    pub task: String,
    pub rubric: Option<String>,
}

/// Persist a trial's produced output (phase artifacts + repo diff) + task/rubric
/// under `trials/<id>/`, so `review` can show it offline.
pub fn save_output(
    flows_dir: &Path,
    trial_id: &str,
    output: &str,
    meta: &TrialMeta,
) -> Result<(), String> {
    let dir = trial_dir(flows_dir, trial_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("output.md"), output).map_err(|e| e.to_string())?;
    let meta_json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("meta.json"), meta_json).map_err(|e| e.to_string())
}

/// Read the saved output for a trial (empty string if none).
pub fn read_output(flows_dir: &Path, trial_id: &str) -> String {
    std::fs::read_to_string(trial_dir(flows_dir, trial_id).join("output.md")).unwrap_or_default()
}

/// Read the saved task/rubric for a trial.
pub fn read_meta(flows_dir: &Path, trial_id: &str) -> Option<TrialMeta> {
    let text = std::fs::read_to_string(trial_dir(flows_dir, trial_id).join("meta.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Load all records folded by trial id (latest record per id wins), preserving
/// first-seen order. Malformed lines are skipped.
pub fn load(flows_dir: &Path) -> Vec<TrialRecord> {
    let text = match std::fs::read_to_string(results_path(flows_dir)) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<TrialRecord> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<TrialRecord>(line) else {
            continue;
        };
        match idx.get(&rec.trial) {
            Some(&i) => out[i] = rec, // latest wins, keeps first-seen position
            None => {
                idx.insert(rec.trial.clone(), out.len());
                out.push(rec);
            }
        }
    }
    out
}

/// Current unix epoch seconds.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, manual: Option<bool>) -> TrialRecord {
        TrialRecord {
            trial: id.into(),
            ts: 1,
            model: "qwen".into(),
            config: "baseline".into(),
            fixture: "hello".into(),
            suite: "coder".into(),
            size: "small".into(),
            granularity: "flow".into(),
            verify_pass: Some(true),
            manual_pass: manual,
            manual_score: manual.map(|_| 4),
            note: None,
            in_tok: 100,
            out_tok: 20,
            peak_ctx: 120,
            cost: 0.0,
            rounds: 2,
            secs: 1.0,
        }
    }

    #[test]
    fn append_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &rec("a", None)).unwrap();
        append(dir.path(), &rec("b", None)).unwrap();
        let all = load(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].trial, "a");
        assert_eq!(all[1].trial, "b");
    }

    #[test]
    fn later_grade_overlays_by_id() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &rec("a", None)).unwrap();
        // A later record with the same id (now graded) overrides the earlier one.
        append(dir.path(), &rec("a", Some(true))).unwrap();
        let all = load(dir.path());
        assert_eq!(all.len(), 1); // folded by id
        assert_eq!(all[0].manual_pass, Some(true));
        assert_eq!(all[0].manual_score, Some(4));
    }

    #[test]
    fn save_and_read_output() {
        let dir = tempfile::tempdir().unwrap();
        let meta = TrialMeta {
            task: "do the thing".into(),
            rubric: Some("- correct?".into()),
        };
        save_output(dir.path(), "x1", "output body", &meta).unwrap();
        assert_eq!(read_output(dir.path(), "x1"), "output body");
        assert_eq!(read_meta(dir.path(), "x1").unwrap().task, "do the thing");
    }
}
