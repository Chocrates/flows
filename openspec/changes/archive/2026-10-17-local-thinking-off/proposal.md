## Why

`thinking: false` only affects Anthropic models today. On an OpenAI-compatible local server (vLLM / SGLang serving Qwen3) it does nothing, so a phase cannot opt out of reasoning. In the e2e run a 27B model spent 3–16k reasoning tokens per round planning a one-function change (50 min for the plan). Probing the server: `chat_template_kwargs.enable_thinking: false` turns reasoning off (0.6 s vs 3.7 s on a trivial prompt); `reasoning_effort` and a `thinking_budget` kwarg are ignored.

## What Changes

- When the resolved `thinking` for a phase is `false`, requests to an OpenAI-compatible endpoint other than `api.openai.com` carry `chat_template_kwargs: {enable_thinking: false}`.
- Default behaviour is unchanged (thinking defaults on, and nothing is sent unless it is explicitly off).

## Capabilities

### New Capabilities

### Modified Capabilities
- `model-profiles`: `thinking: false` turns reasoning off on local OpenAI-compatible servers.

## Impact

- `flows-llm` (`CompletionRequest.disable_thinking`, OpenAI provider), `flows-runner::agent`, docs (`DESIGN.md`, configuration).
