//! Ports of the behavior in `tests/test_tools_plugin.py`.
//!
//! Execution tests need a functional bwrap; they skip where userns is
//! unavailable. Load/render tests always run.

use flows_tools::plugin::{list_plugin_tools, load_plugin_tool, run_plugin_tool, PluginToolDef};
use flows_tools::sandbox::sandbox_available;
use serde_json::json;
use tempfile::tempdir;

fn make_tool(flows_dir: &std::path::Path, name: &str, body: &str) {
    let dir = flows_dir.join("tools");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.yml")), body).unwrap();
}

#[test]
fn load_missing_is_none() {
    let d = tempdir().unwrap();
    assert!(load_plugin_tool("nope", Some(d.path())).is_none());
}

#[test]
fn load_returns_def() {
    let d = tempdir().unwrap();
    make_tool(
        d.path(),
        "greet",
        "name: greet\ndescription: Says hi\nparameters:\n  type: object\n  properties:\n    who:\n      type: string\ncommand: 'echo hi {{ who }}'\n",
    );
    let def = load_plugin_tool("greet", Some(d.path())).unwrap();
    assert_eq!(def.name, "greet");
    assert_eq!(def.description, "Says hi");
    assert_eq!(def.parameters["properties"]["who"]["type"], "string");
    assert!(def.command.contains("{{ who }}"));
}

#[test]
fn load_name_defaults_to_filename() {
    let d = tempdir().unwrap();
    make_tool(d.path(), "mytool", "description: x\ncommand: echo hi\n");
    assert_eq!(
        load_plugin_tool("mytool", Some(d.path())).unwrap().name,
        "mytool"
    );
}

#[test]
fn load_parameters_default() {
    let d = tempdir().unwrap();
    make_tool(d.path(), "noop", "name: noop\ncommand: 'true'\n");
    let def = load_plugin_tool("noop", Some(d.path())).unwrap();
    assert_eq!(def.parameters, json!({"type": "object", "properties": {}}));
}

#[test]
fn list_returns_sorted_stems() {
    let d = tempdir().unwrap();
    for n in ["zebra", "apple", "mango"] {
        make_tool(d.path(), n, "name: x\n");
    }
    assert_eq!(
        list_plugin_tools(Some(d.path())),
        vec!["apple", "mango", "zebra"]
    );
}

#[test]
fn list_empty_when_no_dir() {
    let d = tempdir().unwrap();
    assert!(list_plugin_tools(Some(d.path())).is_empty());
}

#[test]
fn empty_command_errors() {
    let d = tempdir().unwrap();
    let def = PluginToolDef {
        name: "e".into(),
        description: String::new(),
        parameters: json!({}),
        command: String::new(),
        expose_credentials: false,
    };
    let err = run_plugin_tool(&def, &json!({}), d.path()).unwrap_err();
    assert!(err.0.contains("no command"));
}

#[test]
fn bad_template_errors() {
    let d = tempdir().unwrap();
    let def = PluginToolDef {
        name: "bad".into(),
        description: String::new(),
        parameters: json!({}),
        command: "echo {{ ".into(), // unterminated
        expose_credentials: false,
    };
    let err = run_plugin_tool(&def, &json!({}), d.path()).unwrap_err();
    assert!(err.0.contains("render failed"));
}

// -- execution (needs bwrap) -------------------------------------------------

#[test]
fn renders_and_runs() {
    if !sandbox_available() {
        eprintln!("SKIP renders_and_runs: no functional bwrap");
        return;
    }
    let d = tempdir().unwrap();
    let def = PluginToolDef {
        name: "greet".into(),
        description: String::new(),
        parameters: json!({}),
        command: "echo hi {{ who }}".into(),
        expose_credentials: false,
    };
    let out = run_plugin_tool(&def, &json!({"who": "world"}), d.path()).unwrap();
    assert!(out.contains("hi world"));
}

#[test]
fn sh_quote_filter_escapes_arg() {
    if !sandbox_available() {
        eprintln!("SKIP sh_quote_filter_escapes_arg: no functional bwrap");
        return;
    }
    let d = tempdir().unwrap();
    let def = PluginToolDef {
        name: "echo".into(),
        description: String::new(),
        parameters: json!({}),
        command: "echo {{ msg | sh_quote }}".into(),
        expose_credentials: false,
    };
    // Shell-injection payload: WITHOUT sh_quote, `; touch pwned` would run as a
    // second command and create the file. WITH it, the whole string is one quoted
    // literal argument to `echo`.
    let payload = "; touch pwned #";
    let out = run_plugin_tool(&def, &json!({"msg": payload}), d.path()).unwrap();
    // Echoed literally...
    assert!(
        out.contains("; touch pwned"),
        "payload not echoed literally: {out:?}"
    );
    // ...and the injected command did NOT execute.
    assert!(
        !d.path().join("pwned").exists(),
        "sh_quote failed to neutralize the injection — `touch pwned` ran"
    );
}
