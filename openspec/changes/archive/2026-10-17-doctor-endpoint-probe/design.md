## Context

Doctor resolves the default model and each profile and runs `preflight_llm` (config-only checks). The web-search check already probes SearXNG with a runtime it builds itself.

## Goals / Non-Goals

**Goals:** catch a down server, a wrong `api_base` or a wrong model name before a run.

**Non-Goals:** probing cloud APIs; a completion probe (a single-slot server busy with a run would block it); probing in `flows run` pre-flight.

## Decisions

- **`flows_llm::list_models(api_base, api_key, timeout)`** does `GET {base}/models` (bearer auth when a key is set) and returns the `data[].id` list. It lives in `flows-llm` so the CLI holds no HTTP logic.
- **Errors are categorised for the user**: timeout ("no answer in 5s — down, or busy"), connection failure, non-2xx status, unparseable body.
- **Matching** is exact on the id; when the id is not listed doctor warns and prints up to five listed ids. llama.cpp lists a file name and ignores the requested model, so this is a warning, not an error.
- **One probe per endpoint**: profiles sharing an `api_base` + key reuse one listing.

## Risks / Trade-offs

- [A server without `/models`] → reported as an HTTP error naming the status; harmless.
