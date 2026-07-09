## Why

`flows` is built to run on small, local models (llama.cpp) that have narrow context windows, yet it targets real — often large — codebases. Today every agent prompt dumps a flat 500-entry repo tree and lets the model navigate only by `list_files` + whole-file `read_file`, with no search. On a big repo the tree is truncated noise, whole-file reads blow the window, and a small model gets lost. Better context delivery is the difference between "unusable on my repo" and "works offline on a 14B coder model."

## What Changes

Delivered in three tiers so value lands early:

- **Tier 1 — find & read precisely (cheap, high value)**
  - Add a `search_code` tool: repo-scoped, ripgrep-style text/symbol search returning `file:line: match` so the model can *find* relevant code instead of guessing.
  - Give `read_file` optional `start_line` / `end_line` and a default byte/line cap, so the model reads slices, not 2000-line files.
  - Replace the flat 500-entry `repo_tree` with a gitignore-aware, depth-bounded, size-capped tree that points the model at `search_code` / `list_files` to explore.
- **Tier 2 — a map of the whole repo (medium effort)**
  - Add a **repo map**: per source file, its key symbols (fn / struct / class / type signatures), ranked by relevance to the idea, injected in place of / alongside the slim tree — a symbol-level index of the codebase without full files.
  - Add **agent-loop context budgeting**: using the already-measured context-window size, truncate/summarize old tool results and stale rounds when input tokens approach a configured fraction of the window, so the loop never overflows.
- **Tier 3 — semantic retrieval (heavier, best relevance)**
  - Add embedding-based retrieval: index repo chunks and inject the top-k most relevant to the idea/phase. Local-first — embeddings run against an OpenAI-compatible `/v1/embeddings` endpoint (llama.cpp), no cloud required. Opt-in via config.
- **Cross-cutting cleanup**
  - Scope per-phase context: stop injecting the full `prior_artifacts` map *and* each key (current double-injection); give each phase only the artifacts/context it needs.
- **Gating & evaluation** (so we can prove each tier earns its tokens)
  - Put every feature behind an independent `context.*` flag, defaulting to the current baseline behavior, with a per-run override so configs can be A/B'd without editing global config.
  - Add an eval harness (`flows eval`): a labeled fixtures corpus + a config-permutation runner that reports, per config, retrieval quality (recall@k, deterministic) and end-to-end outcomes (task success, tokens, peak context size, cost, tool-rounds) — baseline vs each flag, ablated and cumulative.

Non-goals: changing the phase/pipeline model, the sandbox, or the on-disk run formats. No new required network dependency (Tier 3 is opt-in and local-capable).

## Capabilities

### New Capabilities
- `code-search`: repo-scoped code search (`search_code`) and range-aware file reads that let an agent locate and read exactly the code it needs.
- `repo-context`: assembling the per-prompt repository context — a bounded repo tree, a ranked symbol-level repo map, per-phase context scoping, and context-window budgeting/trimming in the agent loop.
- `semantic-retrieval`: optional embedding-based retrieval of the most relevant repo chunks for a phase, using a local/OpenAI-compatible embeddings endpoint.
- `context-eval`: independent feature gating (`context.*` flags + per-run override) and an eval harness that measures retrieval quality and end-to-end task outcomes per configuration, so each tier's usefulness is quantified against the baseline.

### Modified Capabilities
<!-- No existing spec's requirements change; security-model invariants are preserved (search/read stay repo-scoped, embeddings honor the SSRF/network guards). -->

## Impact

- **Code**: `flows-runner` (`agent.rs` context assembly + tool defs/dispatch, `repo_context.rs` tree → map, new budgeting in the tool loop, new retrieval module); `flows-tools` (`files.rs` range reads, new `search.rs`); `flows-llm` (embeddings request path for Tier 3); `flows-core` (config for retrieval + budgeting knobs).
- **Prompts**: template variables gain `repo_map` / retrieved-context; existing `repo_tree` slimmed. Prompt templates in `flows-runner/assets/prompts` updated to use them.
- **Dependencies**: Tier 1 search via the `ignore` + `regex` crates (ripgrep core, no external binary); Tier 2 symbol extraction via **tree-sitter** grammars (rust, python, js/ts, go) — reused by Tier 3 chunking; Tier 3 a small on-disk vector index cached under `~/.flows`, embeddings via an OpenAI-compatible `/v1/embeddings` endpoint.
- **Security**: `search_code` and range reads are repo-scoped (same path guards as file tools); embeddings requests go through the existing SSRF-guarded HTTP path. `security-model` invariants preserved.
- **Config/formats**: additive config keys only; no change to `state.json` / artifacts / `llm.jsonl`.
