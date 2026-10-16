//! Port of `flows/tools/plugin.py` — user-defined tools from
//! `~/.flows/tools/<name>.yml`. The command is a MiniJinja template rendered
//! with the call args, then run through the sandbox (LLM-supplied args must not
//! be able to escape the repo).

use std::path::{Path, PathBuf};

use minijinja::{Environment, Value as JinjaValue};
use serde_json::Value;

use crate::error::{ToolError, ToolResult};
use crate::sandbox::{run_confined, Limits, DEFAULT_MAX_OUTPUT, DEFAULT_TIMEOUT_SECS};

#[derive(Debug, Clone, PartialEq)]
pub struct PluginToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema object.
    pub parameters: Value,
    /// Jinja shell-command template.
    pub command: String,
}

fn flows_base(flows_dir: Option<&Path>) -> PathBuf {
    flows_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
            .join(".flows")
    })
}

pub fn load_plugin_tool(name: &str, flows_dir: Option<&Path>) -> Option<PluginToolDef> {
    let path = flows_base(flows_dir)
        .join("tools")
        .join(format!("{name}.yml"));
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let raw: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).ok()?;
    let get_str = |k: &str| raw.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let parameters = raw
        .get("parameters")
        .and_then(|p| serde_json::to_value(p).ok())
        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
    Some(PluginToolDef {
        name: get_str("name").unwrap_or_else(|| name.to_string()),
        description: get_str("description").unwrap_or_default(),
        parameters,
        command: get_str("command").unwrap_or_default(),
    })
}

pub fn list_plugin_tools(flows_dir: Option<&Path>) -> Vec<String> {
    let dir = flows_base(flows_dir).join("tools");
    let mut names: Vec<String> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("yml"))
                    .then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .flatten()
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

fn sh_quote(value: String) -> String {
    // shlex::try_quote is Cow; always succeeds for normal strings
    shlex::try_quote(&value)
        .map(|c| c.into_owned())
        .unwrap_or(value)
}

fn render_command(template: &str, args: &Value) -> ToolResult<String> {
    let mut env = Environment::new();
    env.add_filter("sh_quote", sh_quote);
    env.add_template("cmd", template)
        .map_err(|e| ToolError::new(format!("command render failed: {e}")))?;
    let tmpl = env.get_template("cmd").unwrap();
    let ctx = JinjaValue::from_serialize(args);
    tmpl.render(ctx)
        .map_err(|e| ToolError::new(format!("command render failed: {e}")))
}

pub fn run_plugin_tool(defn: &PluginToolDef, args: &Value, repo_root: &Path) -> ToolResult<String> {
    if defn.command.is_empty() {
        return Err(ToolError::new(format!(
            "Plugin tool {:?} has no command defined",
            defn.name
        )));
    }
    let command = render_command(&defn.command, args)?;
    // Confine to the project dir, exactly like the bash tool.
    run_confined(
        &command,
        repo_root,
        DEFAULT_TIMEOUT_SECS,
        DEFAULT_MAX_OUTPUT,
        &Limits::default(),
    )
}
