# flows — agent working context

## What this project is

`flows` is a local-first pipeline automation tool. It chains LLM agents, shell scripts, and human checkpoints into repeatable workflows — all run in-process against the project directory, all state on disk. No cloud orchestration, no Docker, no Temporal.

It is a **Rust** Cargo workspace producing a single `flows` binary. (It was ported from Python; `docs/RUST_PORT.md` is the historical port plan — the Python package no longer exists.)

## Stack

| Layer | Crate(s) |
|-------|----------|
| Async runtime / HTTP | `tokio`, `reqwest` (rustls) |
| LLM calls | hand-rolled providers in `flows-llm`: Anthropic Messages + OpenAI-compatible Chat Completions (streaming SSE, tool calls, reasoning) |
| CLI | `clap` (derive) + `clap_complete` |
| TUI | `ratatui` + `crossterm`, `arboard` clipboard |
| Prompt templates / loop conditions | `minijinja` (Jinja2-compatible) + Markdown |
| Config / state | `serde`, `serde_json`, `serde_yaml_ng` |
| Repo context | `ignore` (gitignore-aware walk), tree-sitter (repo map), `fastembed` (local ONNX embeddings, opt-in retrieval) |
| Web search | DuckDuckGo (default), self-hosted SearXNG, keyed Brave / Serper, doc lookup |
| Shell sandbox | bubblewrap (Linux) / Seatbelt (macOS) — external binaries |
| Tests | `cargo test`, `portable-pty` + `vt100` for real-terminal TUI tests |

## Project layout

```
flows/
├── Cargo.toml                    — workspace (version, shared deps)
├── Makefile                      — fmt / fmt-check / lint / test / check / install
├── crates/
│   ├── flows-core/               — no I/O-heavy logic; shared types + config
│   │   ├── src/models.rs         — PhaseConfig, ToolsConfig, Artifact, GlobalConfig, presets
│   │   ├── src/config.rs         — 3-tier config loader + merge, `context` block
│   │   ├── src/templates.rs      — template lookup (project → global → embedded builtin)
│   │   ├── src/state.rs          — atomic run state, signal files (`Store`)
│   │   ├── src/fan_out.rs        — `for_each` groups: item normalization/ordering, expansion
│   │   ├── src/{pricing,usage,estimate,dryrun,detect,notify,llm_log,thinking,pipeline,json}.rs
│   │   └── assets/default_templates/*.yml — builtin templates (embedded)
│   ├── flows-llm/                — `Provider` trait; anthropic.rs, openai.rs, stream.rs, retry.rs
│   ├── flows-tools/              — tools the agent can call
│   │   ├── src/files.rs          — read_file (+ line ranges), write_file, list_files (repo-scoped, O_NOFOLLOW)
│   │   ├── src/code_search.rs    — search_code (repo-scoped regex search)
│   │   ├── src/bash.rs           — allowlisted commands, run through the sandbox
│   │   ├── src/sandbox/          — `run_confined`: bwrap.rs (Linux), seatbelt.rs (macOS); fails closed
│   │   ├── src/http.rs           — http_get with SSRF guard + redirect revalidation
│   │   ├── src/{search,lookup,html}.rs — web search backends, doc lookup
│   │   └── src/plugin.rs         — user plugin tools (~/.flows/tools/*.yml)
│   ├── flows-runner/             — execution engine
│   │   ├── src/runner.rs         — run_pipeline (linear + DAG), RunnerCallbacks, cost guard
│   │   ├── src/agent.rs          — LLM tool loop, nudges/stall detection, context assembly
│   │   ├── src/script.rs         — script phases (+ `@auto:test` / `@auto:lint`)
│   │   ├── src/prompts.rs        — prompt lookup + MiniJinja render
│   │   ├── src/{repo_context,repo_map,budget,retrieval}.rs — context delivery for small models
│   │   └── assets/prompts/<template>/<phase>.md — builtin prompts (embedded)
│   ├── flows-tui/                — ratatui app: live run, attach view, home/launcher screen
│   └── flows-cli/                — the `flows` binary (main.rs) + eval harness (eval*.rs), searxng.rs
├── eval-fixtures/                — labeled fixtures for `flows eval`
├── openspec/                     — specs (openspec/specs/) + change proposals (openspec/changes/)
├── docs/                         — architecture, configuration, eval, templates, ROADMAP
├── DESIGN.md                     — architecture and design decisions
└── README.md                     — user-facing documentation
```

Dependency direction: `flows-core` ← `flows-tools`, `flows-llm` ← `flows-runner` ← `flows-tui` ← `flows-cli`.

## Build & test commands

Use the `make` targets (CI runs the same gate):

```bash
make check          # fmt-check + clippy -D warnings + cargo test --workspace (full gate)
make fmt            # cargo fmt --all
make lint           # cargo clippy --workspace --all-targets -- -D warnings
make test           # cargo test --workspace
make install        # install `flows` to ~/.cargo/bin

cargo test -p flows-runner --test agent   # single test file
cargo run -p flows-cli -- --help          # CLI entrypoint without installing
```

A few tests are `#[ignore]`d (e.g. end-to-end retrieval that downloads an embedding model); run them with `cargo test -- --ignored`.

## CLI commands

`doctor`, `config`, `init`, `templates`, `plan` (validate + cost estimate), `run [--tui|--background] [-C dir]`, `runs`, `attach`, `usage`, `stop`, `delete`, `clean`, `searxng up|down|status`, `eval run|list|review|report|clean|retrieval`, `completions`, and the hidden `worker` (background run process). Running `flows` with no args opens the TUI home screen.

## Config system (3-tier, lowest → highest priority)

```
~/.flows/config.yml               # global user: model, api_key, profiles, notifications, context
~/.flows/projects/<repo-name>.yml # user project override: model, cost limit
.flows.yml                        # repo project config (committed)
```

For LLM config: project wins over user-project-override wins over global.
`max_cost_usd` and `notifications` live in global/user-project layers only.
Named model **profiles** let each phase route to a different model/provider.
The `context` block gates context-delivery features (all off by default). Project keys overlay global keys (`config::merge_context`); `flows run --context map+rag` overrides the flags for one run (`config::apply_context_override`, passed to background workers via `FLOWS_CONTEXT`).

## Runner architecture

`run_pipeline()` in `flows-runner/src/runner.rs` is the core execution engine. It:
1. Validates the pipeline and writes `RunMeta` / `RunState` to `~/.flows/runs/{run_id}/`
2. Merges LLM config (global → project) and resolves per-phase model profiles
3. Detects execution mode:
   - **Linear**: no phase has `depends_on` → sequential, with `loop_back_to` and `on_failure: retry:X`; `for_each` groups (`flows-core/src/fan_out.rs`) expand at run time into `<group>.<id>.<phase>` phases spliced after the group
   - **DAG**: any phase has `depends_on` → ready phases run concurrently. `loop_back_to` / `retry:X` are rejected at validation. Human phases wait for in-flight tasks (serialized).
4. Runs each phase (agent / script / human)
5. Fires `RunnerCallbacks` events throughout

### RunnerCallbacks

All UI (TUI, foreground print, background worker) is implemented via the `RunnerCallbacks` trait. Core crates must not print — report through callbacks (`on_note`, `on_status`, …). Only `flows-cli` and `flows-tui` write to the terminal.

### Loop conditions

Loop conditions are MiniJinja expressions evaluated in a sandboxed environment against the artifact content (`content`). `true` → loop fires.

## Phase types

| Type | Where | Key behavior |
|------|-------|-------------|
| `agent` | `flows-runner/src/agent.rs` | LLM loop, tool calls, clarification rounds, streaming, stall guards (no-op writes, empty responses, max rounds), cost cap |
| `script` | `flows-runner/src/script.rs` | subprocess, MiniJinja-rendered command, captures stdout/stderr |
| `human` | `flows-runner/src/runner.rs` | awaits user input; in background mode polls signal files |

## Tool system

Tools are enabled per phase via `ToolsConfig`. Presets: `files`, `files_and_test`, `files_and_deps`, `research`, `research_ro`, `research_and_code`, `all`. Plugin tools: `~/.flows/tools/<name>.yml`. Models with `supports_tools: false` get no tool schemas and receive pushed context instead.

Every shell command (bash tool + plugin tools) runs through `sandbox::run_confined`: filesystem read-only except the project dir and a private `/tmp`. If no sandbox is available, commands refuse to run. Bash additionally requires an allowlist; hardcoded blocks apply regardless (`rm -rf /`, `curl|bash`, `sudo`, writes to `/etc/`, fork bombs).

## Prompt loading (`flows-runner/src/prompts.rs`)

Lookup order (first match wins):
1. `phase.prompt` field (repo-relative path or builtin key)
2. `<repo>/.flows/prompts/{template}/{phase}.md` (project override)
3. `~/.flows/prompts/{template}/{phase}.md` (global override)
4. builtin `assets/prompts/{template}/{phase}.md`
5. `<repo>/.flows/prompts/{phase}.md` → 6. `~/.flows/prompts/{phase}.md` → 7. builtin generic
8. Auto-generated fallback

## Template loading (`flows-core/src/templates.rs`)

`<repo>/.flows/templates/<name>.yml` → `~/.flows/templates/<name>.yml` → embedded builtin → error.

## State on disk

```
~/.flows/runs/{run_id}/
  meta.json      — idea, repo_path, template, phases, created_at, pid
  state.json     — status, current_phase, completed_phases, questions, error
  llm.jsonl      — every LLM call (request, response, tool calls, usage)
  output.log     — streamed output (read by `flows attach`)
  artifacts/{phase}.yml
  signals/       — cancel, advance, answer.txt
~/.flows/index/<repo-hash>/<model>.json — persisted retrieval index
```

State files are written atomically (temp file + rename).

## Specs (OpenSpec)

Capabilities live in `openspec/specs/` (agent-loop-guards, code-search, conditional-phases, context-eval, eval-harness, model-io, model-profiles, phase-fan-out, pipeline-composition, repo-context, security-model, semantic-retrieval, template-registry, web-search). New features go through a change under `openspec/changes/` (propose → apply → archive). Use the `opsx:*` / `openspec-*` skills.

## Hard constraints

- **Gate must pass before committing:** `make check` (fmt, clippy `-D warnings`, all tests).
- **Every change ships with tests.** Agent-loop behavior is tested with the mock provider in `crates/flows-runner/tests/agent.rs`.
- **No printing in core crates** (`flows-core`, `flows-tools`, `flows-llm`, `flows-runner`). Use `RunnerCallbacks`.
- **Model agnostic.** All LLM calls go through the `flows-llm` `Provider` trait; never call a vendor API from elsewhere.
- **No business logic in the CLI.** `flows-cli` calls into core/runner functions.
- **File tools scoped to repo root.** Traversal, absolute paths, and symlink escapes are rejected.
- **Shell commands are sandboxed and fail closed.** `BashConfig.unrestricted = true` requires explicit opt-in.
- **New context features land behind a `context.*` flag**, off by default.

## Common patterns

### Adding a new phase type
1. Add a variant to `PhaseType` in `flows-core/src/models.rs` (+ parsing/validation)
2. Implement and dispatch it in `flows-runner/src/runner.rs`
3. Add tests in `crates/flows-runner/tests/`

### Adding a new tool
1. Implement in `flows-tools/src/your_tool.rs` (+ `tests/your_tool.rs`)
2. Add a field to `ToolsConfig` in `flows-core/src/models.rs` and update presets if needed
3. Register the schema in `build_tool_defs` and handle it in `dispatch_tool` in `flows-runner/src/agent.rs`

### Adding a new CLI command
Add a variant to `Command` in `flows-cli/src/main.rs`; put the logic in a library crate and test it there.

### Adding a new template
Drop `<name>.yml` in `crates/flows-core/assets/default_templates/` and its prompts in `crates/flows-runner/assets/prompts/<name>/`. Both are embedded at build time.
