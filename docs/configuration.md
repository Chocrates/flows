# Configuration

Global config lives in `~/.flows/config.yml`; a per-project `.flows.yml` overrides
it. Set values with `flows config set`, or edit the file. Scaffold a fully-commented
starter with `flows init --global`.

```yaml
llm:
  model: qwen2.5-coder                 # any name; llama.cpp serves whatever's loaded
  api_base: http://localhost:8080/v1   # your OpenAI-compatible endpoint
  thinking: false
max_cost_usd: 0.0                      # 0 = no cap (local is free anyway)
notifications:
  enabled: true
  sound: false
```

For a **cloud** model, drop `api_base` and set `model: claude-sonnet-4-6` (with
`ANTHROPIC_API_KEY`) or a `gpt-*` model (with `OPENAI_API_KEY`).

- **Provider is chosen by model name:** `claude*` → Anthropic; anything else →
  OpenAI Chat Completions (what llama.cpp speaks). An `api_base` routes to your
  local/self-hosted endpoint — **no key required**.
- **API keys** (cloud only): `llm.api_key`, else `$ANTHROPIC_API_KEY`, else
  `$OPENAI_API_KEY`.

Generate configs:

```bash
flows init            # writes .flows.yml (detects Python / Rust / Node / Go)
flows init --global   # scaffold ~/.flows/config.yml + sync templates + prompts
```

---

## Per-phase models (model profiles)

Different phases can run on different models and endpoints. Define named
**profiles** under `models:`, then point a phase at one with `model:`:

```yaml
llm:
  model: claude-sonnet-4-6             # the run default

models:
  reasoner:                            # a cloud reasoner for planning/design
    model: claude-opus-4-8
    thinking: true
  coder:                               # a local coder for implementation
    model: qwen2.5-coder
    api_base: http://localhost:8080/v1

phases:
  - name: design
    model: reasoner
  - name: implement
    model: coder                       # different model AND endpoint
```

- A profile carries its own `model` and optional `api_base`, `api_key`,
  `thinking`; omit `api_base` to inherit the default endpoint (two models on the
  same endpoint just need a `model:`).
- A `phase.model` / `llm.model` that **doesn't** name a profile is treated as a
  literal model string on the default endpoint.
- Each profile's key is its explicit `api_key`, else the dialect default. `flows
  plan` shows the model each phase resolves to; `flows doctor` flags an unusable
  profile before you run.

### Models that can't call tools

Some reasoning models don't do function calling. Mark the profile
`supports_tools: false`:

```yaml
models:
  reasoner:
    model: deepseek-r1
    supports_tools: false
```

flows then sends **no tool schemas** and instead instructs the model to emit file
writes inline in its artifact (`content.files`: path → contents), which flows
writes after the response. Such a model can't read the repo, so prefer a
tool-capable model when the task depends on reading existing code (or use the
push-style [context features](#context-delivery-for-small-models)).

---

## Search & lookup tools

Agent phases with a research/implement preset get three ways to reach outside the
repo:

- **`lookup`** — structured queries against source-specific JSON APIs. Reliable,
  no setup, no key. Sources: `crates_io`, `docs_rs`, `stackexchange`, `wikipedia`,
  `github` (uses `GITHUB_TOKEN` if set). Prefer this for library/API details —
  e.g. `lookup(source="crates_io", query="bevy")` returns the latest version.
- **`http_get`** — fetch a public URL (SSRF-guarded).
- **`web_search`** — general web search. Four backends:

```yaml
tools:
  web_search:
    backend: duckduckgo      # default — HTML scrape, no setup (can be rate-limited)
    max_results: 5
    # …or a self-hosted SearXNG instance:
    # backend: searxng
    # base_url: http://localhost:8888
    # api_key: ""            # only if the instance gates its JSON API
    # …or a keyed API (more reliable than scraping, costs quota):
    # backend: brave         # Brave Search API — key: api_key or $BRAVE_API_KEY
    # backend: serper        # Serper (Google results) — key: api_key or $SERPER_API_KEY
    # base_url: …            # optional endpoint override for brave/serper
```

Keyed backends send the key only to their own endpoint (or `base_url`).
`flows doctor` reports whether a key is configured; it doesn't make a request,
so it doesn't spend quota. A 401/403 reply means the key is wrong, and 429 means
you're over the rate limit or quota.

### Self-hosted SearXNG

Public SearXNG instances block scripted JSON access, so run your own. flows drives
any available container engine (docker / podman / nerdctl / finch):

```bash
flows searxng up        # detect an engine, run searxng on :8888 (JSON enabled)
flows searxng status
flows searxng down
```

Then set `backend: searxng` + `base_url: http://localhost:8888` above. `up` writes
a hand-editable `~/.flows/searxng/settings.yml` (JSON format on, bot-limiter off).

---

## Context delivery for small models

Small local models have narrow context windows but real repos are large. These
features give a model the *right* code without blowing the window — some **pull**
(the model calls a tool), some **push** (injected up front, so they help even
models that can't function-call). All live under a `context:` block and **default
off**; enable per feature:

```yaml
context:
  search_tool: true          # `search_code` tool: repo-scoped ripgrep-style search (pull)
  range_reads: true          # read_file(path, start_line, end_line) — read slices (pull)
  slim_tree: true            # bounded, gitignore-aware repo tree (push)
  repo_map: true             # tree-sitter symbol map, ranked by the task (push)
  budgeting: true            # trim old tool results as the window fills
  retrieval: true            # local embedding retrieval of relevant chunks (push)
  # tuning:
  retrieval_model: minilm            # local embed model (empty → all-MiniLM-L6-v2)
  retrieval_max_chunks: 8            # hard cap on injected chunks
  retrieval_budget_fraction: 0.25    # retrieved code ≤ this × the model's window
  retrieval_reindex_threshold: 0.4   # ≥40% files changed → rebuild the index
  budget_fraction: 0.6               # trim when est. tokens exceed this × window
  unknown_window_tokens: 8000        # assumed window when the model's is unknown
  repo_map_max_tokens: 1500
```

- The block can live in `~/.flows/config.yml`, `.flows.yml`, or both. Keys the
  project sets override the global ones; everything else is inherited (so a
  project can say just `retrieval: true` and keep your global `retrieval_model`).
- Try a combination for one run without editing config:
  `flows run "…" --context map+rag` (tokens: `search`, `range`, `slim`, `map`,
  `budget`, `rag`; or `all` / `baseline`). It replaces the on/off flags only;
  tuning keys still come from config.
- A `supports_tools: false` model gets **no** tool schemas — only push context —
  so it never wastes turns trying to call tools.
- **Retrieval injects whole definitions.** Code is chunked at function/type
  boundaries (Rust, Python, JS/TS, Go; other files use 50-line windows) and the
  best matches are packed into `retrieval_budget_fraction` of the model's
  context window (or `unknown_window_tokens`), at most `retrieval_max_chunks`.
- **Retrieval is fully local.** Embeddings run in-process (`fastembed`/ONNX, a
  small model downloaded once) — no server, no cloud, no key. The index is built
  once per flow, persisted under `~/.flows/index/`, and rebuilt only when enough
  files change.
- **Measure before enabling.** Use [`flows eval`](./eval.md) to A/B a config
  against baseline on your model. "Useful" is a measured delta, not an assumption.
