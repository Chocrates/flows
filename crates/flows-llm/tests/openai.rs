//! OpenAI Chat Completions translation tests (pure).

use flows_llm::openai::{build_body, parse_response};
use flows_llm::types::{CompletionRequest, Message, ToolCall, ToolDef};
use serde_json::json;

fn req(messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        model: "gpt-4o".into(),
        system: "You are helpful.".into(),
        messages,
        tools: vec![],
        max_tokens: 4096,
        thinking_budget: None,
        no_tool_calls: false,
        disable_thinking: false,
    }
}

#[test]
fn system_prepended_as_message() {
    let body = build_body(&req(vec![Message::user("hi")]));
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[0]["content"], "You are helpful.");
    assert_eq!(msgs[1]["role"], "user");
    assert_eq!(body["max_tokens"], 4096);
}

#[test]
fn assistant_tool_call_null_content() {
    let tc = ToolCall {
        id: "t1".into(),
        name: "bash".into(),
        arguments: r#"{"command":"ls"}"#.into(),
    };
    let body = build_body(&req(vec![Message::assistant_tools(None, vec![tc])]));
    let m = &body["messages"][1];
    assert_eq!(m["content"], serde_json::Value::Null);
    assert_eq!(m["tool_calls"][0]["function"]["name"], "bash");
    assert_eq!(m["tool_calls"][0]["type"], "function");
}

#[test]
fn tool_result_role() {
    let body = build_body(&req(vec![Message::tool_result("t1", "output")]));
    let m = &body["messages"][1];
    assert_eq!(m["role"], "tool");
    assert_eq!(m["tool_call_id"], "t1");
    assert_eq!(m["content"], "output");
}

#[test]
fn tools_wrapped_in_function_type() {
    let mut r = req(vec![Message::user("hi")]);
    r.tools = vec![ToolDef {
        name: "read_file".into(),
        description: "read".into(),
        parameters: json!({"type": "object"}),
    }];
    let body = build_body(&r);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    assert!(body.get("tool_choice").is_none());
    r.no_tool_calls = true;
    assert_eq!(build_body(&r)["tool_choice"], "none");
}

#[test]
fn reasoning_model_uses_max_completion_tokens() {
    let mut r = req(vec![Message::user("hi")]);
    r.model = "o3-mini".into();
    let body = build_body(&r);
    assert_eq!(body["max_completion_tokens"], 4096);
    assert!(body.get("max_tokens").is_none());
}

#[test]
fn parse_content_and_usage() {
    let resp = json!({
        "choices": [{"message": {"content": "Hello!"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    let c = parse_response(&resp).unwrap();
    assert_eq!(c.content, "Hello!");
    assert_eq!(c.usage.input_tokens, 10);
    assert_eq!(c.usage.output_tokens, 5);
    assert_eq!(c.stop_reason.as_deref(), Some("stop"));
}

#[test]
fn parse_tool_calls() {
    let resp = json!({
        "choices": [{"message": {"content": null, "tool_calls": [
            {"id": "t1", "type": "function", "function": {"name": "bash", "arguments": "{\"command\":\"ls\"}"}}
        ]}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    });
    let c = parse_response(&resp).unwrap();
    assert_eq!(c.tool_calls.len(), 1);
    assert_eq!(c.tool_calls[0].name, "bash");
    let args: serde_json::Value = serde_json::from_str(&c.tool_calls[0].arguments).unwrap();
    assert_eq!(args["command"], "ls");
}

#[test]
fn parse_missing_message_errors() {
    assert!(parse_response(&json!({"choices": []})).is_err());
}

// -- streaming (SSE chunks) --------------------------------------------------

use flows_llm::openai::StreamAccumulator;
use std::sync::Mutex;

fn stream(events: &[serde_json::Value]) -> (flows_llm::types::Completion, Vec<String>) {
    let tokens = Mutex::new(Vec::new());
    let mut acc = StreamAccumulator::new();
    let on_token = |t: &str| tokens.lock().unwrap().push(t.to_string());
    let noop = |_: &str| {};
    for e in events {
        acc.process(e, &on_token, &noop);
    }
    (acc.finish(), tokens.into_inner().unwrap())
}

#[test]
fn stream_accumulates_content_and_usage() {
    let events = vec![
        json!({"choices": [{"delta": {"content": "Hel"}}]}),
        json!({"choices": [{"delta": {"content": "lo"}}]}),
        json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        json!({"choices": [], "usage": {"prompt_tokens": 7, "completion_tokens": 2}}),
    ];
    let (c, tokens) = stream(&events);
    assert_eq!(c.content, "Hello");
    assert_eq!(tokens, vec!["Hel", "lo"]);
    assert_eq!(c.usage.input_tokens, 7);
    assert_eq!(c.usage.output_tokens, 2);
    assert_eq!(c.stop_reason.as_deref(), Some("stop"));
}

#[test]
fn stream_assembles_tool_call_across_chunks() {
    let events = vec![
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "t1", "function": {"name": "bash"}}]}}]}),
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{\"command\":"}}]}}]}),
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": " \"ls\"}"}}]}}]}),
    ];
    let (c, _) = stream(&events);
    assert_eq!(c.tool_calls.len(), 1);
    assert_eq!(c.tool_calls[0].id, "t1");
    assert_eq!(c.tool_calls[0].name, "bash");
    let args: serde_json::Value = serde_json::from_str(&c.tool_calls[0].arguments).unwrap();
    assert_eq!(args["command"], "ls");
}

#[test]
fn stream_empty_tool_args_default() {
    let events = vec![
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "t", "function": {"name": "noop"}}]}}]}),
    ];
    let (c, _) = stream(&events);
    assert_eq!(c.tool_calls[0].arguments, "{}");
}

#[test]
fn stream_captures_reasoning_content() {
    let tokens = Mutex::new(Vec::new());
    let thinking = Mutex::new(Vec::new());
    let mut acc = StreamAccumulator::new();
    let on_t = |t: &str| tokens.lock().unwrap().push(t.to_string());
    let on_r = |t: &str| thinking.lock().unwrap().push(t.to_string());
    for e in [
        json!({"choices": [{"delta": {"reasoning_content": "let me think"}}]}),
        json!({"choices": [{"delta": {"content": "answer"}}]}),
    ] {
        acc.process(&e, &on_t, &on_r);
    }
    let c = acc.finish();
    assert_eq!(c.reasoning.as_deref(), Some("let me think"));
    assert_eq!(c.content, "answer");
    assert_eq!(*thinking.lock().unwrap(), vec!["let me think"]);
}

#[test]
fn parse_reads_reasoning_content() {
    let resp = json!({
        "choices": [{"message": {"content": "hi", "reasoning_content": "hmm"}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    });
    let c = parse_response(&resp).unwrap();
    assert_eq!(c.reasoning.as_deref(), Some("hmm"));
}

#[test]
fn thinking_off_only_reaches_local_servers() {
    use flows_llm::openai::apply_endpoint_options;
    let mut r = req(vec![Message::user("hi")]);
    let local = "http://192.168.1.2:8000/v1";
    let mut body = build_body(&r);
    apply_endpoint_options(&mut body, &r, local);
    assert!(
        body.get("chat_template_kwargs").is_none(),
        "default sends nothing"
    );

    r.disable_thinking = true;
    let mut body = build_body(&r);
    apply_endpoint_options(&mut body, &r, local);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);

    let mut body = build_body(&r);
    apply_endpoint_options(&mut body, &r, "https://api.openai.com/v1");
    assert!(
        body.get("chat_template_kwargs").is_none(),
        "official API rejects it"
    );
}
