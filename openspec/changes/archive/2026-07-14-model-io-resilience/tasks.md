## 1. Retry with backoff (flows-llm)

- [x] 1.1 Add a `retry` module: `RetryConfig` (env-tunable via `FLOWS_LLM_MAX_RETRIES`/`_RETRY_BASE_MS`/`_RETRY_MAX_MS`), retryable-status classification (408/425/429/5xx), exponential backoff + jitter with cap. Tests: classification, backoff growth/cap, default first-wait, env override.
- [x] 1.2 Route all four send sites (Anthropic + OpenAI × complete + streaming) through `send_with_retry` — retrying send+status only, before any streamed body is read.

## 2. No-tools models (flows-core + flows-runner)

- [x] 2.1 `LLMConfig.supports_tools: Option<bool>`; parse + carry through profile resolution/merge.
- [x] 2.2 Agent: when `supports_tools == Some(false)`, send no tool schemas and append a "no tools" instruction (return artifact; inline files in `content.files`). On the final artifact, write each `content.files` entry to disk. Live-verified end-to-end.

## 3. Token cap (flows-core + flows-runner)

- [x] 3.1 `PhaseConfig.max_tokens: Option<u32>`; parse. Agent uses it, else a generous default (raised from 4096). Reasoner templates set a high value.

## 4. Root-path tolerance (flows-tools)

- [x] 4.1 `list_files` treats `""`/`"."`/`"/"` (and a leading `/`) as the repo root; `safe_path` still rejects empty/escaping paths. Test `list_files_root_spellings`.

## 5. Wrap-up

- [x] 5.1 Gate green (fmt, clippy -D warnings, tests); README documents `supports_tools`, `max_tokens`, and the retry env vars.
- [x] 5.2 `openspec validate model-io-resilience`.
