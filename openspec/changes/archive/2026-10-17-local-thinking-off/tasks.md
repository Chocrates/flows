## 1. Implementation

- [x] 1.1 `CompletionRequest.disable_thinking`, set by the agent from the resolved flag
- [x] 1.2 OpenAI provider adds `chat_template_kwargs.enable_thinking: false` for non-official endpoints

## 2. Tests and docs

- [x] 2.1 Provider body tests (local, official, default) and an agent test that the flag reaches the request
- [x] 2.2 DESIGN.md thinking section and configuration docs
