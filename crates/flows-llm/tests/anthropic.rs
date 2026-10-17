//! Anthropic translation tests (pure — no network).

use flows_llm::anthropic::{build_body, parse_response};
use flows_llm::types::{CompletionRequest, Message, ToolCall, ToolDef};
use serde_json::json;

fn req(messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        model: "claude-sonnet-4-6".into(),
        system: "You are helpful.".into(),
        messages,
        tools: vec![],
        max_tokens: 4096,
        thinking_budget: None,
        no_tool_calls: false,
        disable_thinking: false,
    }
}

// -- build_body --------------------------------------------------------------

#[test]
fn system_is_top_level_field() {
    let body = build_body(&req(vec![Message::user("hi")]));
    assert_eq!(body["system"], "You are helpful.");
    assert_eq!(body["model"], "claude-sonnet-4-6");
    assert_eq!(body["max_tokens"], 4096);
}

#[test]
fn user_message_becomes_text_block() {
    let body = build_body(&req(vec![Message::user("hello")]));
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"][0]["type"], "text");
    assert_eq!(msgs[0]["content"][0]["text"], "hello");
}

#[test]
fn assistant_tool_only_turn_has_no_empty_text_block() {
    // Regression target: the litellm empty-content placeholder bug does not
    // exist in the native format — a tool-only turn emits only tool_use.
    let tc = ToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        arguments: r#"{"path":"a.txt"}"#.into(),
    };
    let body = build_body(&req(vec![Message::assistant_tools(None, vec![tc])]));
    let blocks = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "tool_use");
    assert_eq!(blocks[0]["name"], "read_file");
    assert_eq!(blocks[0]["input"]["path"], "a.txt");
}

#[test]
fn assistant_with_text_and_tool() {
    let tc = ToolCall {
        id: "t1".into(),
        name: "bash".into(),
        arguments: r#"{"command":"ls"}"#.into(),
    };
    let body = build_body(&req(vec![Message::assistant_tools(
        Some("let me look".into()),
        vec![tc],
    )]));
    let blocks = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[1]["type"], "tool_use");
}

#[test]
fn consecutive_tool_results_merge_into_one_user_turn() {
    let msgs = vec![
        Message::tool_result("t1", "result one"),
        Message::tool_result("t2", "result two"),
    ];
    let body = build_body(&req(msgs));
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(
        arr.len(),
        1,
        "two tool results should merge into one user turn"
    );
    assert_eq!(arr[0]["role"], "user");
    let blocks = arr[0]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["tool_use_id"], "t1");
    assert_eq!(blocks[1]["tool_use_id"], "t2");
}

#[test]
fn tools_serialized_with_input_schema() {
    let mut r = req(vec![Message::user("hi")]);
    r.tools = vec![ToolDef {
        name: "read_file".into(),
        description: "read a file".into(),
        parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }];
    let body = build_body(&r);
    let t = &body["tools"][0];
    assert_eq!(t["name"], "read_file");
    assert_eq!(t["input_schema"]["properties"]["path"]["type"], "string");
}

#[test]
fn thinking_enables_field_and_bumps_max_tokens() {
    let mut r = req(vec![Message::user("hi")]);
    r.thinking_budget = Some(2048);
    r.max_tokens = 1000; // smaller than budget → must be bumped
    let body = build_body(&r);
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 2048);
    assert_eq!(body["max_tokens"], 2048 + 4096);
}

// -- parse_response ----------------------------------------------------------

#[test]
fn parse_text_and_usage() {
    let resp = json!({
        "content": [{"type": "text", "text": "Hello!"}],
        "usage": {"input_tokens": 10, "output_tokens": 5},
        "stop_reason": "end_turn"
    });
    let c = parse_response(&resp).unwrap();
    assert_eq!(c.content, "Hello!");
    assert_eq!(c.usage.input_tokens, 10);
    assert_eq!(c.usage.output_tokens, 5);
    assert_eq!(c.stop_reason.as_deref(), Some("end_turn"));
    assert!(c.reasoning.is_none());
    assert!(c.tool_calls.is_empty());
}

#[test]
fn parse_thinking_into_reasoning() {
    let resp = json!({
        "content": [
            {"type": "thinking", "thinking": "let me consider..."},
            {"type": "text", "text": "answer"}
        ],
        "usage": {"input_tokens": 1, "output_tokens": 1}
    });
    let c = parse_response(&resp).unwrap();
    assert_eq!(c.reasoning.as_deref(), Some("let me consider..."));
    assert_eq!(c.content, "answer");
}

#[test]
fn parse_tool_use() {
    let resp = json!({
        "content": [{"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "ls"}}],
        "usage": {"input_tokens": 1, "output_tokens": 1}
    });
    let c = parse_response(&resp).unwrap();
    assert_eq!(c.tool_calls.len(), 1);
    assert_eq!(c.tool_calls[0].id, "t1");
    assert_eq!(c.tool_calls[0].name, "bash");
    let args: serde_json::Value = serde_json::from_str(&c.tool_calls[0].arguments).unwrap();
    assert_eq!(args["command"], "ls");
}

#[test]
fn parse_missing_content_errors() {
    let resp = json!({"usage": {"input_tokens": 1, "output_tokens": 1}});
    assert!(parse_response(&resp).is_err());
}

#[test]
fn no_tool_calls_keeps_tools_but_forbids_calls() {
    let mut r = req(vec![Message::user("hi")]);
    r.tools = vec![ToolDef {
        name: "read_file".into(),
        description: "read".into(),
        parameters: json!({"type": "object"}),
    }];
    assert!(build_body(&r).get("tool_choice").is_none());
    r.no_tool_calls = true;
    let body = build_body(&r);
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["tool_choice"], json!({"type": "none"}));
}
