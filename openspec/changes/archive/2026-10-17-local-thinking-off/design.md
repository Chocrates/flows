## Context

`resolve_thinking` yields `(enabled, budget)`; only Anthropic-family models get a budget. The OpenAI provider ignores thinking entirely.

## Goals / Non-Goals

**Goals:** let `thinking: false` (phase, profile or llm layer) switch reasoning off on Qwen-style chat templates.

**Non-Goals:** budgets or effort levels for local servers (the probed server ignores them); changing the default.

## Decisions

- **Signal**: `CompletionRequest.disable_thinking`, set from the resolved flag, so providers decide how to express it.
- **Where it is sent**: the OpenAI provider adds `chat_template_kwargs` only when its base URL is not `api.openai.com` — the official API rejects unknown parameters, and its reasoning models are not switched off this way. Anthropic needs nothing: no budget means no thinking.
- **Templates that do not know the kwarg** ignore it (Jinja chat templates ignore unknown variables), so sending it to a non-Qwen local model is harmless.

## Risks / Trade-offs

- [A gateway in front of api.openai.com under another host rejects the field] → it is only sent when the user explicitly set `thinking: false`.
