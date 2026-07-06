//! Port of `flows/templates.py` — built-in pipeline templates + user overrides.
//!
//! Lookup order for `load_template(name)`:
//!   1. `<repo_root>/.flows/templates/<name>.yml`  (project override)
//!   2. `<flows_dir>/templates/<name>.yml`          (global user override)
//!   3. embedded builtin                            (shipped defaults)

use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};

use crate::config::{parse_phases_from_yaml, ConfigError};
use crate::models::PhaseConfig;

/// Shipped default templates, embedded at compile time.
static BUILTIN: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/default_templates");

const DEFAULT_TEMPLATE: &str = "feature";

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("Unknown template: {name:?}. Available: {available:?}")]
    Unknown {
        name: String,
        available: Vec<String>,
    },
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

fn global_templates_dir(flows_dir: Option<&Path>) -> PathBuf {
    flows_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".flows")
        })
        .join("templates")
}

fn yml_stems(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("yml") {
                    p.file_stem().map(|s| s.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// (filename, contents) for every embedded builtin template — used by
/// `templates sync` to write the shipped defaults into `~/.flows/templates/`.
pub fn builtin_files() -> Vec<(String, &'static str)> {
    BUILTIN
        .files()
        .filter_map(|f| {
            let name = f.path().file_name()?.to_string_lossy().into_owned();
            Some((name, f.contents_utf8().unwrap_or_default()))
        })
        .collect()
}

fn builtin_stems() -> Vec<String> {
    let mut names: Vec<String> = BUILTIN
        .files()
        .filter_map(|f| {
            let p = f.path();
            if p.extension().and_then(|x| x.to_str()) == Some("yml") {
                p.file_stem().map(|s| s.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// All available template names (project → global → builtin), de-duplicated
/// with first-seen order preserved.
pub fn list_templates(flows_dir: Option<&Path>, repo_root: Option<&Path>) -> Vec<String> {
    let mut ordered: Vec<String> = Vec::new();
    let push = |name: String, ordered: &mut Vec<String>| {
        if !ordered.contains(&name) {
            ordered.push(name);
        }
    };

    if let Some(repo) = repo_root {
        for n in yml_stems(&repo.join(".flows").join("templates")) {
            push(n, &mut ordered);
        }
    }
    for n in yml_stems(&global_templates_dir(flows_dir)) {
        push(n, &mut ordered);
    }
    for n in builtin_stems() {
        push(n, &mut ordered);
    }
    ordered
}

fn load_from_str(text: &str) -> Result<(Vec<PhaseConfig>, String), TemplateError> {
    let phases = parse_phases_from_yaml(text)?;
    Ok((phases, text.to_string()))
}

/// Load a template's phases + raw YAML text. `name = None` uses the default
/// (`feature`). Project override → global override → embedded builtin.
pub fn load_template(
    name: Option<&str>,
    repo_root: &Path,
    flows_dir: Option<&Path>,
) -> Result<(Vec<PhaseConfig>, String), TemplateError> {
    let resolved = name.unwrap_or(DEFAULT_TEMPLATE);
    let file = format!("{resolved}.yml");

    let project_file = repo_root.join(".flows").join("templates").join(&file);
    let global_file = global_templates_dir(flows_dir).join(&file);

    let text = if project_file.exists() {
        read_file(&project_file)?
    } else if global_file.exists() {
        read_file(&global_file)?
    } else if let Some(f) = BUILTIN.get_file(&file) {
        f.contents_utf8().unwrap_or_default().to_string()
    } else {
        return Err(TemplateError::Unknown {
            name: resolved.to_string(),
            available: list_templates(flows_dir, None),
        });
    };

    load_from_str(&text)
}

fn read_file(path: &Path) -> Result<String, TemplateError> {
    std::fs::read_to_string(path).map_err(|source| TemplateError::Io {
        path: path.display().to_string(),
        source,
    })
}
