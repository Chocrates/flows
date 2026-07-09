//! Port of `flows/state.py` — run directories, atomic state/meta writes, and
//! signal files (advance/cancel/answer).
//!
//! Python uses a module-global `_FLOWS_DIR` that tests monkeypatch. Here the
//! flows dir is owned by a `Store`, so it's explicit and trivially testable;
//! `Store::home()` reproduces the `~/.flows` default.

use std::path::{Path, PathBuf};

use crate::models::{RunMeta, RunState};

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("json error at {path}: {source}")]
    Json {
        path: String,
        source: serde_json::Error,
    },
}

type Result<T> = std::result::Result<T, StateError>;

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> StateError + '_ {
    move |source| StateError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// On-disk run store rooted at a flows dir (`~/.flows` by default).
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The default `~/.flows` store.
    pub fn home() -> Self {
        Self {
            root: home_dir().join(".flows"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    // -- run dirs --------------------------------------------------------

    pub fn runs_dir(&self) -> Result<PathBuf> {
        let d = self.root.join("runs");
        std::fs::create_dir_all(&d).map_err(io_err(&d))?;
        Ok(d)
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join("runs").join(run_id)
    }

    pub fn ensure_run_dir(&self, run_id: &str) -> Result<PathBuf> {
        let d = self.run_dir(run_id);
        for sub in ["artifacts", "signals"] {
            let p = d.join(sub);
            std::fs::create_dir_all(&p).map_err(io_err(&p))?;
        }
        Ok(d)
    }

    /// Delete a run's directory (idempotent — no error if it's already gone).
    pub fn delete_run(&self, run_id: &str) -> Result<()> {
        let d = self.run_dir(run_id);
        if d.exists() {
            std::fs::remove_dir_all(&d).map_err(io_err(&d))?;
        }
        Ok(())
    }

    // -- atomic json -----------------------------------------------------

    fn write_json<T: serde::Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        let body = serde_json::to_string_pretty(value).map_err(|source| StateError::Json {
            path: path.display().to_string(),
            source,
        })?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, body).map_err(io_err(&tmp))?;
        std::fs::rename(&tmp, path).map_err(io_err(path))?;
        Ok(())
    }

    fn read_json<T: serde::de::DeserializeOwned>(&self, path: &Path) -> Result<T> {
        let text = std::fs::read_to_string(path).map_err(io_err(path))?;
        serde_json::from_str(&text).map_err(|source| StateError::Json {
            path: path.display().to_string(),
            source,
        })
    }

    // -- meta / state ----------------------------------------------------

    pub fn write_meta(&self, meta: &RunMeta) -> Result<()> {
        self.write_json(&self.run_dir(&meta.run_id).join("meta.json"), meta)
    }

    pub fn read_meta(&self, run_id: &str) -> Result<RunMeta> {
        self.read_json(&self.run_dir(run_id).join("meta.json"))
    }

    pub fn write_state(&self, state: &RunState) -> Result<()> {
        self.write_json(&self.run_dir(&state.run_id).join("state.json"), state)
    }

    pub fn read_state(&self, run_id: &str) -> Result<Option<RunState>> {
        let path = self.run_dir(run_id).join("state.json");
        if !path.exists() {
            return Ok(None);
        }
        self.read_json(&path).map(Some)
    }

    // -- signals ---------------------------------------------------------

    pub fn signal_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("signals")
    }

    fn touch(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(io_err(path))?;
        Ok(())
    }

    pub fn send_advance(&self, run_id: &str) -> Result<()> {
        self.touch(&self.signal_dir(run_id).join("advance"))
    }

    pub fn send_cancel(&self, run_id: &str) -> Result<()> {
        self.touch(&self.signal_dir(run_id).join("cancel"))
    }

    pub fn send_answer(&self, run_id: &str, text: &str) -> Result<()> {
        let dir = self.signal_dir(run_id);
        std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        let answer = dir.join("answer.txt");
        std::fs::write(&answer, text).map_err(io_err(&answer))?;
        self.touch(&dir.join("advance"))
    }

    /// Consume the `advance` signal if present.
    pub fn poll_advance(&self, run_id: &str) -> bool {
        self.consume(&self.signal_dir(run_id).join("advance"))
    }

    /// Consume the `cancel` signal if present.
    pub fn poll_cancel(&self, run_id: &str) -> bool {
        self.consume(&self.signal_dir(run_id).join("cancel"))
    }

    /// Peek at the `cancel` signal without consuming it (for mid-phase checks;
    /// the between-phase `poll_cancel` still consumes it to end the run).
    pub fn is_cancelled(&self, run_id: &str) -> bool {
        self.signal_dir(run_id).join("cancel").exists()
    }

    fn consume(&self, path: &Path) -> bool {
        if path.exists() {
            let _ = std::fs::remove_file(path);
            true
        } else {
            false
        }
    }

    /// Read and consume the pending answer, if any.
    pub fn read_answer(&self, run_id: &str) -> Option<String> {
        let path = self.signal_dir(run_id).join("answer.txt");
        if !path.exists() {
            return None;
        }
        let text = std::fs::read_to_string(&path).ok()?.trim().to_string();
        let _ = std::fs::remove_file(&path);
        Some(text)
    }

    // -- artifacts / listing --------------------------------------------

    pub fn artifact_path(&self, run_id: &str, phase_name: &str) -> PathBuf {
        self.run_dir(run_id)
            .join("artifacts")
            .join(format!("{phase_name}.yml"))
    }

    /// All runs that have a readable meta.json, newest first (by mtime).
    pub fn list_runs(&self) -> Vec<RunMeta> {
        let runs = self.root.join("runs");
        let mut dirs: Vec<(std::time::SystemTime, String)> = match std::fs::read_dir(&runs) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let mtime = e.metadata().and_then(|m| m.modified()).ok()?;
                    Some((mtime, e.file_name().to_string_lossy().into_owned()))
                })
                .collect(),
            Err(_) => return Vec::new(),
        };
        dirs.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime)); // newest first
        dirs.into_iter()
            .filter_map(|(_, name)| self.read_meta(&name).ok())
            .collect()
    }
}
