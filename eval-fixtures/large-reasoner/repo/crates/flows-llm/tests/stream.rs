//! Anthropic streaming accumulator tests (pure — fixture events, no network).

use std::sync::Mutex;

use flows_llm::stream::Accumulator;
use serde_json::{json, Value};

fn run(events: &[Value]) -> (flows_llm::types::Completion, Vec<String>, Vec<String>) {
    let tokens = Mutex::new(Vec::new());
    let thinking = Mutex::new(Vec::new());
    let mut acc = Accumulator::new();
    let on_token = |t: &str| tokens.lock().unwrap().push(t.to_string());
    let on_thinking = |t: &str| thinking.lock().unwrap().push(t.to_string());
    for ev in events {
        acc.process(ev, &on_token, &on_thinking);
    }
    let completion = acc.finish();
    (
        completion,
        tokens.into_inner().unwrap(),
        thinking.into_inner().unwrap(),
    )
}

#[test]
fn accumulates_text_and_emits_tokens() {
    let events = vec![
        json!({"type": "message_start", "message": {"usage": {"input_tokens": 12}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hel"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "lo"}}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 3}}),
    ];
    let (c, tokens, _) = run(&events);
    assert_eq!(c.content, "Hello");
    assert_eq!(tokens, vec!["Hel", "lo"]);
    assert_eq!(c.usage.input_tokens, 12);
    assert_eq!(c.usage.output_tokens, 3);
    assert_eq!(c.stop_reason.as_deref(), Some("end_turn"));
}

#[test]
fn accumulates_thinking() {
    let events = vec![
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm "}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "ok"}}),
    ];
    let (c, tokens, thinking) = run(&events);
    assert_eq!(c.reasoning.as_deref(), Some("hmm ok"));
    assert!(tokens.is_empty()); // thinking is not a content token
    assert_eq!(thinking, vec!["hmm ", "ok"]);
}

#[test]
fn assembles_tool_call_from_input_json_deltas() {
    let events = vec![
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t1", "name": "bash"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"command\":"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": " \"ls\"}"}}),
    ];
    let (c, _, _) = run(&events);
    assert_eq!(c.tool_calls.len(), 1);
    assert_eq!(c.tool_calls[0].id, "t1");
    assert_eq!(c.tool_calls[0].name, "bash");
    let args: Value = serde_json::from_str(&c.tool_calls[0].arguments).unwrap();
    assert_eq!(args["command"], "ls");
}

#[test]
fn text_then_tool_call_interleaved() {
    let events = vec![
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "let me look"}}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "t2", "name": "read_file"}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"path\":\"a.txt\"}"}}),
    ];
    let (c, tokens, _) = run(&events);
    assert_eq!(c.content, "let me look");
    assert_eq!(tokens, vec!["let me look"]);
    assert_eq!(c.tool_calls.len(), 1);
    let args: Value = serde_json::from_str(&c.tool_calls[0].arguments).unwrap();
    assert_eq!(args["path"], "a.txt");
}

#[test]
fn empty_tool_input_defaults_to_object() {
    let events = vec![
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t3", "name": "noop"}}),
    ];
    let (c, _, _) = run(&events);
    assert_eq!(c.tool_calls[0].arguments, "{}");
}
