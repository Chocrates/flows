//! Port of `flows/detect.py` — infer language/test/lint for config generation.

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct StackInfo {
    pub language: String,
    pub test_cmd: String,
    pub lint_cmd: String,
    pub bash_allowed: Vec<String>,
    pub detected: Vec<String>,
}

impl Default for StackInfo {
    fn default() -> Self {
        Self {
            language: "unknown".to_string(),
            test_cmd: String::new(),
            lint_cmd: String::new(),
            bash_allowed: Vec::new(),
            detected: Vec::new(),
        }
    }
}

fn exists(root: &Path, name: &str) -> bool {
    root.join(name).exists()
}

fn is_dir(root: &Path, name: &str) -> bool {
    root.join(name).is_dir()
}

fn read(root: &Path, name: &str) -> String {
    std::fs::read_to_string(root.join(name)).unwrap_or_default()
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Scan a project root and infer language, test runner, and linter.
pub fn detect_stack(root: &Path) -> StackInfo {
    let mut info = StackInfo::default();

    // Python
    let has_pyproject = exists(root, "pyproject.toml");
    if has_pyproject || exists(root, "setup.py") || exists(root, "requirements.txt") {
        info.language = "python".into();
        info.detected.push("Python".into());

        let pyproject_text = if has_pyproject {
            read(root, "pyproject.toml")
        } else {
            String::new()
        };

        let pm = if exists(root, "uv.lock") {
            "uv"
        } else if exists(root, "poetry.lock") {
            "poetry"
        } else {
            "pip"
        };
        info.detected.push(pm.into());

        let has_pytest = pyproject_text.contains("pytest")
            || exists(root, "pytest.ini")
            || exists(root, "conftest.py")
            || is_dir(root, "tests")
            || is_dir(root, "test");
        info.test_cmd = if has_pytest {
            "pytest -x -q".into()
        } else {
            format!("{pm} run test")
        };
        if has_pytest {
            info.detected.push("pytest".into());
        }

        let mut linters: Vec<String> = Vec::new();
        if pyproject_text.contains("ruff") {
            linters.push("ruff check . || true".into());
            info.detected.push("ruff".into());
        }
        if pyproject_text.contains("mypy") {
            linters.push("mypy . || true".into());
            info.detected.push("mypy".into());
        }
        if linters.is_empty() && pyproject_text.contains("flake8") {
            linters.push("flake8 . || true".into());
            info.detected.push("flake8".into());
        }
        info.lint_cmd = linters.join(" && ");
        info.bash_allowed = vec![
            format!("{pm}*"),
            "pytest*".into(),
            "python*".into(),
            "make*".into(),
        ];
        return info;
    }

    // Node / JS / TS
    if exists(root, "package.json") {
        info.language = "node".into();
        info.detected.push("Node.js".into());

        let pkg: serde_json::Value =
            serde_json::from_str(&read(root, "package.json")).unwrap_or(serde_json::Value::Null);

        let pm = if exists(root, "pnpm-lock.yaml") {
            "pnpm"
        } else if exists(root, "yarn.lock") {
            "yarn"
        } else {
            "npm"
        };
        info.detected.push(pm.into());

        let scripts = pkg.get("scripts");
        let has = |k: &str| scripts.and_then(|s| s.get(k)).is_some();
        if has("test") {
            info.test_cmd = format!("{pm} test");
            info.detected.push("npm test".into());
        } else if has("test:unit") {
            info.test_cmd = format!("{pm} run test:unit");
        } else {
            info.test_cmd = format!("{pm} test");
        }

        if has("lint") {
            info.lint_cmd = format!("{pm} run lint || true");
            info.detected.push("lint script".into());
        } else if has("typecheck") {
            info.lint_cmd = format!("{pm} run typecheck || true");
        }

        info.bash_allowed = vec![
            format!("{pm}*"),
            "node*".into(),
            "npx*".into(),
            "make*".into(),
        ];
        return info;
    }

    // Rust
    if exists(root, "Cargo.toml") {
        info.language = "rust".into();
        info.detected.push("Rust".into());
        info.test_cmd = "cargo test".into();
        info.lint_cmd = "cargo clippy -- -D warnings || true".into();
        info.bash_allowed = strs(&["cargo*", "rustfmt*", "make*"]);
        return info;
    }

    // Go
    if exists(root, "go.mod") {
        info.language = "go".into();
        info.detected.push("Go".into());
        info.test_cmd = "go test ./...".into();
        info.lint_cmd = "go vet ./... || true".into();
        info.bash_allowed = strs(&["go*", "make*"]);
        return info;
    }

    info.detected.push("unknown stack".into());
    info.bash_allowed = strs(&["make*", "python*", "npm*", "cargo*"]);
    info
}

/// Return a `.flows.yml` string tailored to the detected stack.
pub fn generate_config(info: &StackInfo, template: &str) -> String {
    let header = format!(
        "# .flows.yml — generated (detected: {})",
        info.detected.join(", ")
    );
    // No active `llm:` block: the model comes from ~/.flows/config.yml so a
    // generated project config doesn't override a local (llama.cpp) or cloud
    // model. Uncomment to pin a model for this project.
    let llm_block = "# llm:\n#   model: claude-sonnet-4-6      # else uses ~/.flows/config.yml\n#   api_key: ${ANTHROPIC_API_KEY}";
    let commit_cmd = "git add -A && git commit -m 'feat: {{ idea[:60] }}'";

    if template == "research" {
        return format!(
            r#"{header}
template: research

{llm_block}

phases:
  - name: gather
    type: agent
    tools: research
    max_clarification_rounds: 2

  - name: synthesize
    type: agent
    tools: files
    loop_back_to: gather
    loop_condition: "content.get('needs_more_sources') == True"
    max_loop_iterations: 2

  - name: report
    type: agent
    tools: files

  - name: approve
    type: human
    instructions: "Review the report. Type feedback to revise, or press Enter to accept."
    loop_back_to: report
    loop_condition: "content.get('answer', '').strip() not in ('', 'ok', 'y', 'yes', 'lgtm', 'approve', 'approved')"
    max_loop_iterations: 3
"#
        );
    }

    let allowed_yaml = info
        .bash_allowed
        .iter()
        .map(|p| format!("          - \"{p}\""))
        .collect::<Vec<_>>()
        .join("\n");

    let lint_block = if info.lint_cmd.is_empty() {
        String::new()
    } else {
        format!(
            "\n  - name: lint\n    type: script\n    command: \"{}\"\n    on_failure: retry:implement\n",
            info.lint_cmd
        )
    };

    let test_block = if info.test_cmd.is_empty() {
        String::new()
    } else {
        format!(
            "  - name: test\n    type: script\n    command: \"{}\"\n    on_failure: retry:implement\n",
            info.test_cmd
        )
    };

    format!(
        r#"{header}
template: {template}

{llm_block}

phases:
  - name: plan
    type: agent
    tools: files
    max_clarification_rounds: 3

  - name: implement
    type: agent
    tools:
      files: true
      bash:
        allowed:
{allowed_yaml}
    on_failure: stop
{lint_block}
{test_block}  - name: review
    type: human
    instructions: "Type feedback to request changes, or press Enter to accept."
    loop_back_to: implement
    loop_condition: "content.get('answer', '').strip() not in ('', 'ok', 'y', 'yes', 'lgtm', 'approve', 'approved')"
    max_loop_iterations: 5

  - name: commit
    type: script
    command: "{commit_cmd}"
    on_failure: continue
"#
    )
}
