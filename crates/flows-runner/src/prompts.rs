//! Prompt loading + rendering. Port of `load_prompt` / `render_prompt` from
//! `flows/phases/agent.py`. Builtin prompts are embedded; project/global
//! overrides come from the filesystem.

use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};
use minijinja::Environment;
use serde_json::Value;

use flows_core::models::PhaseConfig;

static BUILTIN: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/prompts");

/// (relative path, contents) for every embedded builtin prompt (e.g.
/// `feature/plan.md`) — used by `templates sync` to write them into
/// `~/.flows/prompts/` so users can customize agent instructions.
pub fn builtin_prompt_files() -> Vec<(String, &'static str)> {
    fn collect(dir: &'static Dir<'static>, out: &mut Vec<(String, &'static str)>) {
        for f in dir.files() {
            if let Some(c) = f.contents_utf8() {
                out.push((f.path().to_string_lossy().into_owned(), c));
            }
        }
        for d in dir.dirs() {
            collect(d, out);
        }
    }
    let mut out = Vec::new();
    collect(&BUILTIN, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Load agent documentation (`AGENTS.md`) — the cross-tool convention for repo
/// instructions. Global `~/.flows/AGENTS.md` (shared conventions) is combined
/// with the project's `<repo>/AGENTS.md` (project wins by coming last), so the
/// agent honours things like "use `make test` for all tooling". Returns None if
/// neither exists.
pub fn load_agent_docs(repo_root: &Path, flows_dir: Option<&Path>) -> Option<String> {
    let flows_root = flows_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home_dir().join(".flows"));
    let mut parts: Vec<String> = Vec::new();
    for path in [flows_root.join("AGENTS.md"), repo_root.join("AGENTS.md")] {
        if let Some(text) = read_fs(&path) {
            let text = text.trim();
            if !text.is_empty() {
                parts.push(text.to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn flows_prompts_dir(flows_dir: Option<&Path>) -> PathBuf {
    flows_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".flows")
        })
        .join("prompts")
}

fn read_fs(path: &Path) -> Option<String> {
    path.exists()
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
}

fn read_builtin(rel: &str) -> Option<String> {
    BUILTIN
        .get_file(rel)
        .and_then(|f| f.contents_utf8())
        .map(str::to_string)
}

/// Resolve the prompt text for a phase (project → global → builtin → fallback).
pub fn load_prompt(
    phase: &PhaseConfig,
    template_name: &str,
    repo_root: &Path,
    flows_dir: Option<&Path>,
) -> String {
    let global_dir = flows_prompts_dir(flows_dir);
    let project_dir = repo_root.join(".flows").join("prompts");

    // 1. Explicit prompt field (repo-relative path or builtin key)
    if let Some(prompt) = &phase.prompt {
        if let Some(text) = read_fs(&repo_root.join(prompt)) {
            return text;
        }
        if let Some(text) = read_builtin(prompt) {
            return text;
        }
    }

    // An included phase is looked up under the template that defined it, by its
    // name there (before any `as:` prefix).
    let (template_name, local_name) = match &phase.origin {
        Some(o) => (o.template.as_str(), o.local_name.as_str()),
        None => (template_name, phase.name.as_str()),
    };
    let file = format!("{local_name}.md");
    if !template_name.is_empty() {
        // 2. project override, 3. global override, 4. builtin — by template
        if let Some(text) = read_fs(&project_dir.join(template_name).join(&file)) {
            return text;
        }
        if let Some(text) = read_fs(&global_dir.join(template_name).join(&file)) {
            return text;
        }
        if let Some(text) = read_builtin(&format!("{template_name}/{file}")) {
            return text;
        }
    }
    // 5. project generic, 6. global generic, 7. builtin generic — by phase name
    if let Some(text) = read_fs(&project_dir.join(&file)) {
        return text;
    }
    if let Some(text) = read_fs(&global_dir.join(&file)) {
        return text;
    }
    if let Some(text) = read_builtin(&file) {
        return text;
    }

    // 8. Fallback. Rendered through `render_prompt`, so the `{{ idea }}` /
    // `{{ repo_tree }}` placeholders are filled from the agent context — a bare
    // phase (e.g. the `blank` template) still gets the task and the repository
    // file tree, not just a generic instruction.
    FALLBACK_PROMPT.replace("__PHASE__", &phase.name)
}

/// Generic single-phase prompt used when no template/phase prompt is found. Uses
/// Jinja placeholders (`{{ ... }}`) filled by [`render_prompt`]; single braces in
/// the JSON examples are literal to Jinja. It carries what a shipped prompt
/// would: earlier phases' artifacts (so `implement` sees the plan), the
/// uncommitted diff, pushed context (repo map, retrieved code) and the tool list.
const FALLBACK_PROMPT: &str =
    "You are an expert assistant running the **__PHASE__** phase of a workflow.\n\n\
# Task\n{{ idea }}\n\n\
{% if prior_artifacts %}# Earlier phases\n\
Output of the phases that ran before this one; build on it.\n\n\
{% for name, art in prior_artifacts | items %}## {{ name }}\n\
```json\n{{ (art.get('content', {}) | tojson(indent=2))[:6000] }}\n```\n\n\
{% endfor %}{% endif %}\
# Repository files\n{{ repo_tree }}\n\n\
{% if changes %}# Uncommitted changes\n```diff\n{{ changes }}\n```\n\n{% endif %}\
{% if repo_map %}# Repository symbol map\n{{ repo_map }}\n\n{% endif %}\
{% if pushed_context %}{{ pushed_context }}\n\n{% endif %}\
{% if tools_enabled %}# Tools available\n{{ tools_list }}\n\n\
Use the tools to inspect and modify the repository as needed to complete the task. \
Paths are repo-relative.\n\n{% endif %}\
When done, produce a structured YAML artifact OR ask clarifying questions if needed. \
Respond with JSON in one of two shapes:\n\
```json\n{\"type\": \"questions\", \"questions\": [\"...\"]}\n```\n\
or\n```json\n{\"type\": \"artifact\", \"content\": {...}}\n```";

/// Render a prompt template with the given context. MiniJinja + the pycompat
/// callback, so the Python-Jinja idioms the shipped prompts use — `.get('k')`,
/// `.get('k', default)`, `.strip()`, `.startswith()` — resolve natively instead
/// of failing (which previously forced a fallback to the raw template).
pub fn render_prompt(template: &str, context: &Value) -> Result<String, String> {
    let mut env = Environment::new();
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    env.render_str(template, minijinja::Value::from_serialize(context))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An included phase finds its prompt under the template that defined it,
    /// by its pre-alias name — not under the including template.
    #[test]
    fn included_phase_uses_origin_template_prompt() {
        let repo = tempfile::tempdir().unwrap();
        let flows = tempfile::tempdir().unwrap();
        let mut p = PhaseConfig::new("second.review");
        p.origin = Some(flows_core::models::IncludeOrigin {
            template: "review".into(),
            local_name: "review".into(),
            prefix: "second.".into(),
            vars: Default::default(),
            fan_out: None,
        });
        let got = load_prompt(&p, "my-template", repo.path(), Some(flows.path()));
        let want = BUILTIN
            .get_file("review/review.md")
            .unwrap()
            .contents_utf8()
            .unwrap();
        assert_eq!(got, want);
    }

    /// The shipped tool-bearing templates gate their tool section on
    /// `tools_enabled`, so a no-tool model is never shown (or told to call) tools.
    #[test]
    fn tool_section_gated_by_tools_enabled() {
        let tmpl = BUILTIN
            .get_file("feature/plan.md")
            .unwrap()
            .contents_utf8()
            .unwrap();
        let on = render_prompt(tmpl, &json!({"idea": "x", "tools_enabled": true})).unwrap();
        let off = render_prompt(tmpl, &json!({"idea": "x", "tools_enabled": false})).unwrap();
        assert!(
            on.contains("## Tools available"),
            "tools on → section shown"
        );
        assert!(
            !off.contains("## Tools available"),
            "tools off → section omitted"
        );
        // The non-tool body still renders either way.
        assert!(off.contains("PLAN ONLY"));
    }

    /// The explain template asks for a prose answer artifact (not a build plan),
    /// so a reasoner rubric has an output shape to grade, and it gates its tool
    /// section like the others.
    #[test]
    fn explain_template_asks_for_answer_and_gates_tools() {
        let tmpl = BUILTIN
            .get_file("feature/explain.md")
            .unwrap()
            .contents_utf8()
            .unwrap();
        let on = render_prompt(
            tmpl,
            &json!({"idea": "how does X work?", "tools_enabled": true}),
        )
        .unwrap();
        let off = render_prompt(
            tmpl,
            &json!({"idea": "how does X work?", "tools_enabled": false}),
        )
        .unwrap();
        // Answer-shaped, not plan-shaped.
        assert!(
            on.contains("\"answer\""),
            "explain artifact needs an answer field"
        );
        assert!(
            !on.contains("files_to_modify"),
            "explain must not be a plan"
        );
        assert!(on.contains("EXPLAIN, do not plan"));
        // Tool section gated.
        assert!(on.contains("## Tools available"));
        assert!(!off.contains("## Tools available"));
    }

    /// Pushed context (oracle / retrieval) is injected when present and absent
    /// otherwise — so baseline prompts are unchanged.
    #[test]
    fn pushed_context_injected_when_present() {
        let tmpl = BUILTIN
            .get_file("feature/explain.md")
            .unwrap()
            .contents_utf8()
            .unwrap();
        let with = render_prompt(
            tmpl,
            &json!({"idea": "q", "pushed_context": "# Reference code\n\nfn foo() {}"}),
        )
        .unwrap();
        let without = render_prompt(tmpl, &json!({"idea": "q"})).unwrap();
        assert!(with.contains("# Reference code") && with.contains("fn foo() {}"));
        assert!(!without.contains("# Reference code"));
    }

    /// plan.md folds reviewer feedback back in on a rework loop (`approve_plan`
    /// feedback present → a "Revise your previous plan" section), and stays silent
    /// on the first pass (no feedback).
    #[test]
    fn plan_reworks_on_reviewer_feedback() {
        let tmpl = BUILTIN
            .get_file("feature/plan.md")
            .unwrap()
            .contents_utf8()
            .unwrap();
        let first = render_prompt(tmpl, &json!({"idea": "x"})).unwrap();
        assert!(!first.contains("Revise your previous plan"));
        let rework = render_prompt(
            tmpl,
            &json!({
                "idea": "x",
                "prior_artifacts": {
                    "plan": {"content": {"summary": "old plan"}},
                    "approve_plan": {"content": {"answer": "use a trait, not an enum"}}
                }
            }),
        )
        .unwrap();
        assert!(rework.contains("Revise your previous plan"));
        assert!(rework.contains("use a trait, not an enum"));
    }

    /// The repo map is injected when present and absent otherwise.
    #[test]
    fn repo_map_injected_when_present() {
        let tmpl = BUILTIN
            .get_file("feature/plan.md")
            .unwrap()
            .contents_utf8()
            .unwrap();
        let with = render_prompt(
            tmpl,
            &json!({"idea": "x", "repo_map": "# Repo map\n### a.rs\n- fn go()"}),
        )
        .unwrap();
        let without = render_prompt(tmpl, &json!({"idea": "x"})).unwrap();
        assert!(with.contains("Repository symbol map") && with.contains("fn go()"));
        assert!(!without.contains("Repository symbol map"));
    }
}
