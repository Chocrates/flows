## Why

`flows doctor` reports where each model routes but never contacts a local endpoint. In the e2e runs a down or misnamed local server only surfaced once a run's first LLM call failed or hung. A cheap `GET /models` probe tells the user up front whether the server answers and whether it serves the configured model.

## What Changes

- `flows doctor` probes every distinct custom OpenAI-compatible endpoint (non-empty `api_base`, non-Claude model) used by the default model or a profile, with a short timeout, and reports: reachable and model listed; reachable but model not listed (with what it does list); or unreachable / timed out / HTTP error.
- Cloud endpoints are not probed (no quota spend, doctor stays usable offline).
- Doctor only reports; it never fails a run.

## Capabilities

### New Capabilities

### Modified Capabilities
- `model-profiles`: the environment check probes local endpoints and flags models they do not serve.

## Impact

- `flows-llm` (new `list_models`), `flows-cli` doctor output, docs.
