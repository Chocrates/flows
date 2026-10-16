## Why

The default DuckDuckGo backend scrapes HTML and gets rate-limited or blocked, and SearXNG needs a self-hosted container. Users with a search API subscription have no reliable, zero-infrastructure option.

## What Changes

- New `web_search` backends `brave` (Brave Search API) and `serper` (Serper's Google results API), mapped to the same `{title, url, snippet}` result shape.
- Key from `tools.web_search.api_key`, else `BRAVE_API_KEY` / `SERPER_API_KEY`; a missing key is a clear error naming both. `base_url` optionally overrides the endpoint.
- HTTP 401/403 and 429 errors carry a hint about the key or the quota.
- `flows doctor` reports key presence for keyed backends without making a request.

## Capabilities

### New Capabilities
- `web-search`: web search backend selection and the keyed backends' behaviour.

### Modified Capabilities
<!-- none -->

## Impact

`crates/flows-tools/src/search.rs`, `crates/flows-cli/src/main.rs` (doctor), docs/configuration.md.
