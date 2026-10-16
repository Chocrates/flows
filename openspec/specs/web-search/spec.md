# web-search Specification

## Purpose
Web search for agents: selectable backends (DuckDuckGo, self-hosted SearXNG, keyed Brave and Serper APIs) behind one `{title, url, snippet}` result shape.
## Requirements
### Requirement: Selectable search backends

The `web_search` tool SHALL support the backends `duckduckgo` (default, no configuration), `searxng` (requires `base_url`), `brave`, and `serper`, selected by `tools.web_search.backend`, and SHALL return at most `max_results` results, each shaped `{title, url, snippet}` regardless of backend. An unknown backend name SHALL produce an error listing the supported ones.

#### Scenario: Same shape from every backend

- **WHEN** a search succeeds on any backend
- **THEN** each result has `title`, `url`, and `snippet` strings, with markup removed from Brave's highlighted text.

#### Scenario: Unknown backend

- **WHEN** `backend` is set to an unsupported name
- **THEN** the tool returns an error naming `duckduckgo`, `searxng`, `brave`, and `serper`.

### Requirement: Keyed search backends

The `brave` and `serper` backends SHALL authenticate with `tools.web_search.api_key`, falling back to the `BRAVE_API_KEY` / `SERPER_API_KEY` environment variable, and SHALL send the key only in the backend's own authentication header (`X-Subscription-Token` / `X-API-KEY`) to its default endpoint or the configured `base_url`. Without a key, the tool SHALL fail with an error naming both the config key and the environment variable. HTTP authentication failures (401/403) and rate limiting (429) SHALL be reported with a hint naming the cause. `flows doctor` SHALL report whether a key is configured without contacting the API.

#### Scenario: Key from environment

- **WHEN** `backend: brave` is set, `api_key` is empty, and `BRAVE_API_KEY` is set
- **THEN** the search uses the environment key.

#### Scenario: Missing key

- **WHEN** `backend: serper` is set and neither `api_key` nor `SERPER_API_KEY` is set
- **THEN** the tool returns an error naming `tools.web_search.api_key` and `$SERPER_API_KEY`, and no request is sent.

#### Scenario: Rejected key

- **WHEN** the API answers 401 or 403
- **THEN** the error includes the status and says to check the API key.

