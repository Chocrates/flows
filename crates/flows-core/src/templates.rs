//! Port of `flows/templates.py` — built-in pipeline templates + user overrides.
//!
//! Lookup order for `load_template(name)`:
//!   1. `<repo_root>/.flows/templates/<name>.yml`  (project override)
//!   2. `<flows_dir>/templates/<name>.yml`          (global user override)
//!   3. embedded builtin                            (shipped defaults)

use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};

use crate::config::{parse_phases_from_yaml_with, ConfigError};
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

/// Load a template's phases + raw YAML text. `name = None` uses the default
/// (`feature`). Project override → global override → embedded builtin.
/// `include:` entries resolve through the same lookup.
pub fn load_template(
    name: Option<&str>,
    repo_root: &Path,
    flows_dir: Option<&Path>,
) -> Result<(Vec<PhaseConfig>, String), TemplateError> {
    let name = name.unwrap_or(DEFAULT_TEMPLATE);
    let text = template_text(name, repo_root, flows_dir)?;
    let mut phases = parse_phases_from_yaml_with(&text, &include_resolver(repo_root, flows_dir))?;
    crate::registry::mark_untrusted(
        &mut phases,
        Some(name),
        &installed_untrusted(repo_root, flows_dir),
    );
    Ok((phases, text))
}

/// Whether template `name` resolves to an untrusted registry install: the
/// global copy wins the lookup (no project override) and the lockfile lists it
/// without trust.
pub fn installed_untrusted<'a>(
    repo_root: &'a Path,
    flows_dir: Option<&'a Path>,
) -> impl Fn(&str) -> bool + 'a {
    let global = global_templates_dir(flows_dir);
    let lock = global
        .parent()
        .map(crate::registry::read_lock)
        .unwrap_or_default();
    move |name: &str| {
        let file = format!("{name}.yml");
        !repo_root
            .join(".flows")
            .join("templates")
            .join(&file)
            .exists()
            && global.join(&file).exists()
            && lock.templates.get(name).is_some_and(|e| !e.trusted)
    }
}

/// A template's raw YAML: project override → global override → builtin.
pub fn template_text(
    name: &str,
    repo_root: &Path,
    flows_dir: Option<&Path>,
) -> Result<String, TemplateError> {
    let file = format!("{name}.yml");
    let project_file = repo_root.join(".flows").join("templates").join(&file);
    let global_file = global_templates_dir(flows_dir).join(&file);

    if project_file.exists() {
        read_file(&project_file)
    } else if global_file.exists() {
        read_file(&global_file)
    } else if let Some(f) = BUILTIN.get_file(&file) {
        Ok(f.contents_utf8().unwrap_or_default().to_string())
    } else {
        Err(TemplateError::Unknown {
            name: name.to_string(),
            available: list_templates(flows_dir, None),
        })
    }
}

/// Resolver for `include:` entries: template names via [`template_text`].
pub fn include_resolver<'a>(
    repo_root: &'a Path,
    flows_dir: Option<&'a Path>,
) -> impl Fn(&str) -> Result<String, String> + 'a {
    move |name| template_text(name, repo_root, flows_dir).map_err(|e| e.to_string())
}

fn read_file(path: &Path) -> Result<String, TemplateError> {
    std::fs::read_to_string(path).map_err(|source| TemplateError::Io {
        path: path.display().to_string(),
        source,
    })
}
