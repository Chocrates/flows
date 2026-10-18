//! Copies of the builtin templates and prompts under `~/.flows/`. A copy there
//! takes precedence over the builtin it came from, so a stale copy silently
//! hides every later fix to that builtin. `synced.json` records the hash of each
//! file `templates sync` wrote; a copy that still has that hash was never edited
//! and can be updated, while an edited one is kept.

use std::collections::BTreeMap;
use std::path::Path;

const MANIFEST: &str = "synced.json";

/// Where a copy under `~/.flows/` stands against its builtin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyState {
    Missing,
    /// Same as the builtin.
    Current,
    /// Unedited since sync wrote it; the builtin has changed since.
    Outdated,
    /// Edited since sync wrote it.
    Edited,
    /// Differs from the builtin, and sync never recorded writing it (copied
    /// before records were kept, or by hand).
    Unrecorded,
}

/// What [`sync_builtins`] did, by path relative to the flows dir.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub written: Vec<String>,
    pub updated: Vec<String>,
    pub kept_edited: Vec<String>,
    pub kept_unrecorded: Vec<String>,
}

/// Every builtin as (path relative to the flows dir, contents):
/// `templates/<name>.yml` and `prompts/<template>/<phase>.md`.
pub fn builtin_copies() -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = flows_core::templates::builtin_files()
        .into_iter()
        .map(|(name, c)| (format!("templates/{name}"), c))
        .collect();
    out.extend(
        crate::prompts::builtin_prompt_files()
            .into_iter()
            .map(|(rel, c)| (format!("prompts/{rel}"), c)),
    );
    out.sort();
    out
}

fn hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

fn load_manifest(flows_dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(flows_dir.join(MANIFEST))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_manifest(flows_dir: &Path, m: &BTreeMap<String, String>) -> Result<(), String> {
    let path = flows_dir.join(MANIFEST);
    let tmp = flows_dir.join(format!("{MANIFEST}.tmp"));
    let body = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn state_of(current: Option<&str>, builtin: &str, recorded: Option<&String>) -> CopyState {
    let Some(current) = current else {
        return CopyState::Missing;
    };
    let h = hash(current);
    if h == hash(builtin) {
        CopyState::Current
    } else if recorded == Some(&h) {
        CopyState::Outdated
    } else if recorded.is_some() {
        CopyState::Edited
    } else {
        CopyState::Unrecorded
    }
}

/// The state of every builtin's copy under `flows_dir`.
pub fn copy_states(flows_dir: &Path) -> Vec<(String, CopyState)> {
    let manifest = load_manifest(flows_dir);
    builtin_copies()
        .into_iter()
        .map(|(rel, builtin)| {
            let current = std::fs::read_to_string(flows_dir.join(&rel)).ok();
            let state = state_of(current.as_deref(), builtin, manifest.get(&rel));
            (rel, state)
        })
        .collect()
}

/// Write the builtins into `flows_dir`: missing copies are written, unedited
/// outdated ones updated, and edited or unrecorded ones kept unless `force`.
pub fn sync_builtins(flows_dir: &Path, force: bool) -> Result<SyncReport, String> {
    let mut manifest = load_manifest(flows_dir);
    let mut report = SyncReport::default();
    for (rel, builtin) in builtin_copies() {
        let path = flows_dir.join(&rel);
        let current = std::fs::read_to_string(&path).ok();
        let state = state_of(current.as_deref(), builtin, manifest.get(&rel));
        let bucket = match (state, force) {
            (CopyState::Current, _) => None,
            (CopyState::Missing, _) => Some(&mut report.written),
            (CopyState::Outdated, _) | (_, true) => Some(&mut report.updated),
            (CopyState::Edited, false) => {
                report.kept_edited.push(rel);
                continue;
            }
            (CopyState::Unrecorded, false) => {
                report.kept_unrecorded.push(rel);
                continue;
            }
        };
        if let Some(bucket) = bucket {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, builtin).map_err(|e| e.to_string())?;
            bucket.push(rel.clone());
        }
        manifest.insert(rel, hash(builtin));
    }
    save_manifest(flows_dir, &manifest)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_follow_the_record() {
        let b = "builtin v2";
        assert_eq!(state_of(None, b, None), CopyState::Missing);
        assert_eq!(state_of(Some(b), b, None), CopyState::Current);
        let v1 = hash("builtin v1");
        assert_eq!(
            state_of(Some("builtin v1"), b, Some(&v1)),
            CopyState::Outdated
        );
        assert_eq!(state_of(Some("mine"), b, Some(&v1)), CopyState::Edited);
        assert_eq!(state_of(Some("mine"), b, None), CopyState::Unrecorded);
    }

    #[test]
    fn sync_updates_unedited_copies_and_keeps_edited_ones() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let first = sync_builtins(dir, false).unwrap();
        let all = builtin_copies();
        assert_eq!(first.written.len(), all.len());
        assert!(copy_states(dir)
            .iter()
            .all(|(_, s)| *s == CopyState::Current));

        // Pretend the builtins moved on: rewrite the record for two copies as
        // if sync had written older contents, and leave one of them unedited.
        let (old_rel, _) = &all[0];
        let (edited_rel, _) = &all[1];
        let (loose_rel, _) = &all[2];
        let mut m = load_manifest(dir);
        std::fs::write(dir.join(old_rel), "old builtin").unwrap();
        m.insert(old_rel.clone(), hash("old builtin"));
        std::fs::write(dir.join(edited_rel), "my edit").unwrap();
        m.insert(edited_rel.clone(), hash("old builtin"));
        std::fs::write(dir.join(loose_rel), "copied by hand").unwrap();
        m.remove(loose_rel);
        save_manifest(dir, &m).unwrap();

        let states: BTreeMap<_, _> = copy_states(dir).into_iter().collect();
        assert_eq!(states[old_rel], CopyState::Outdated);
        assert_eq!(states[edited_rel], CopyState::Edited);
        assert_eq!(states[loose_rel], CopyState::Unrecorded);

        let r = sync_builtins(dir, false).unwrap();
        assert_eq!(r.updated, vec![old_rel.clone()]);
        assert_eq!(r.kept_edited, vec![edited_rel.clone()]);
        assert_eq!(r.kept_unrecorded, vec![loose_rel.clone()]);
        assert_eq!(
            std::fs::read_to_string(dir.join(edited_rel)).unwrap(),
            "my edit"
        );

        let r = sync_builtins(dir, true).unwrap();
        assert_eq!(r.updated.len(), 2);
        assert!(copy_states(dir)
            .iter()
            .all(|(_, s)| *s == CopyState::Current));
    }
}
