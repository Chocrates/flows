# flows — agent working context

## What this project is

`flows` is a local-first pipeline automation tool. It chains LLM agents, shell scripts, and human checkpoints into repeatable workflows — all run in-process against the project directory, all state on disk. No cloud orchestration, no Docker, no Temporal.

## Stack

| Layer | Tool |
|-------|------|
| LLM calls | LiteLLM (any provider, OpenAI-compatible) |
| CLI | Typer + Rich |
| TUI | Textual (live run monitoring, human gates) |
| Prompt templates | Jinja2 + Markdown |
| Config | PyYAML |
| HTTP tool | httpx |
| Web search | duckduckgo-search (default), Brave, Serper |
| Tests | pytest + pytest-asyncio + pytest-cov |

## Project layout

```
flows/
├── flows/
│   ├── cli/main.py          — all CLI commands (excluded from coverage)
│   ├── _worker.py           — background run subprocess (excluded from coverage)
│   ├── tui.py               — Textual TUI (excluded from coverage)
│   ├── runner.py            — pipeline execution engine (linear + DAG)
│   ├── config.py            — 3-tier config loader
│   ├── models.py            — dataclasses: PhaseConfig, ToolsConfig, Artifact, ...
│   ├── templates.py         — template loader (package YAML + user ~/.flows/templates/)
│   ├── detect.py            — stack detection for flows init --generate
│   ├── estimate.py          — cost estimation (no LLM calls)
│   ├── dryrun.py            — dry-run validation and display
│   ├── llm_log.py           — JSONL logger for every LLM call
│   ├── state.py             — atomic read/write of run state on disk
│   ├── usage.py             — parse llm.jsonl, compute cost breakdown
│   ├── pricing.py           — per-model token pricing table
│   ├── notify.py            — cross-platform desktop notifications
│   ├── default_templates/   — builtin template YAML files (feature, software, ...)
│   ├── prompts/             — builtin Jinja2 prompt .md files per template/phase
│   ├── phases/
│   │   ├── agent.py         — LLM loop with tool use
│   │   ├── script.py        — subprocess runner
│   │   └── human.py         — gate waiter
│   └── tools/
│       ├── files.py         — read_file, write_file, list_files (repo-scoped)
│       ├── bash.py          — subprocess with allowlist + hardcoded blocks
│       ├── http.py          — http_get with private IP blocking
│       ├── search.py        — web_search with pluggable backends
│       └── plugin.py        — user-defined plugin tools (~/.flows/tools/*.yml)
├── tests/                   — 427+ tests, 100% coverage required
├── pyproject.toml
├── AGENTS.md                — this file
├── DESIGN.md                — architecture and design decisions
└── README.md                — user-facing documentation
```

## Build & test commands

```bash
uv sync --extra dev          # install all deps including dev
uv run pytest                # all tests (must reach 100% coverage)
uv run pytest tests/test_runner.py  # single file
uv run flows --help          # CLI entrypoint
uv run flows run "idea" --no-tui    # run without TUI (plain output)
```

## Config system (3-tier, lowest → highest priority)

```
~/.flows/config.yml               # global user: model, api_key, notifications
~/.flows/projects/<repo-name>.yml # user project override: model, cost limit
.flows.yml                        # repo project config (committed)
```

For LLM config: project wins over user-project-override wins over global.  
`max_cost_usd` and `notifications` live in global/user-project layers only.

## Runner architecture

`run_pipeline()` in `runner.py` is the core execution engine. It:
1. Writes `RunMeta` and `RunState` to `~/.flows/runs/{run_id}/`
2. Merges LLM config (global → project)
3. Detects execution mode:
   - **Linear**: no phase has `depends_on` → sequential execution with loops and retry
   - **DAG**: any phase has `depends_on` → parallel execution via `asyncio.gather`
4. Calls `_run_phase()` for each phase (agent / script / human)
5. Fires `RunnerCallbacks` events throughout

### RunnerCallbacks

All UI (TUI, foreground print, background notify) is implemented via `RunnerCallbacks`. Never print to stdout in core modules — only in callbacks, cli/main.py, _worker.py, and tui.py.

### Linear vs DAG mode

- **Linear**: phases run sequentially. `loop_back_to`, `on_failure: retry:X` supported.
- **DAG**: phases whose `depends_on` deps are all complete run concurrently via `asyncio.wait`. `loop_back_to` and `retry:X` are NOT supported in DAG mode (validation error at startup).
- Human phases in DAG mode: wait for all in-flight tasks before running (serialized).

### Loop condition evaluation

`_eval_loop_condition(condition, artifact_content)` uses `jinja2.sandbox.SandboxedEnvironment`. Returns `True` → loop fires. Condition is a Jinja2 expression; `content` is the artifact content dict.

## Phase types

| Type | Module | Key behavior |
|------|--------|-------------|
| `agent` | `phases/agent.py` | LLM loop, tool calls, clarification rounds, streaming |
| `script` | `phases/script.py` | subprocess, Jinja2 command render (idea variable), captures stdout/stderr |
| `human` | `phases/human.py` | awaits user input; in background mode polls signal files |

## Tool system

Tools are enabled per phase via `ToolsConfig`. Presets: `files`, `files_and_test`, `files_and_deps`, `research`, `research_and_code`, `all`. Plugin tools: `~/.flows/tools/<name>.yml` loaded by `tools/plugin.py`.

Bash tool requires explicit allowlist. Hardcoded blocks regardless of config: `rm -rf /`, `curl|bash`, `sudo`, writes to `/etc/`, fork bombs.

## Prompt loading (in phases/agent.py)

Lookup order (first match wins):
1. `phase.prompt` field (repo-relative path or builtin key)
2. `~/.flows/prompts/{template}/{phase}.md` (user override)
3. `flows/prompts/{template}/{phase}.md` (builtin)
4. `~/.flows/prompts/{phase}.md` (user generic)
5. `flows/prompts/{phase}.md` (builtin generic)
6. Auto-generated fallback

## Template loading (in templates.py)

Lookup order: `~/.flows/templates/<name>.yml` → `flows/default_templates/<name>.yml` → ValueError.

## State on disk

```
~/.flows/runs/{run_id}/
  meta.json      — idea, repo_path, template, phases, created_at, pid
  state.json     — status, current_phase, completed_phases, questions, error
  llm.jsonl      — every LLM call (request, response, tool calls, usage)
  artifacts/
    {phase}.yml  — each phase's artifact content
  signals/
    cancel       — touch to cancel
    advance      — touch to advance human gate
    answer.txt   — answer text for background Q&A
```

State files written atomically (temp file + rename). Signal files polled by background worker.

## Hard constraints

- **100% test coverage** on all modules except `flows/cli/main.py`, `flows/_worker.py`, `flows/tui.py`. Coverage enforced by `pytest --cov-fail-under=100`.
- **Never print to stdout in core modules.** All output goes through `RunnerCallbacks`.
- **Model agnostic.** All LLM calls via LiteLLM. Never import a provider SDK directly.
- **No business logic in CLI.** CLI calls runner/config/template functions only.
- **File tool scoped to repo root.** `../` traversal blocked in `tools/files.py`.
- **Bash requires allowlist.** `BashConfig.unrestricted=True` requires explicit opt-in.
- Tests must pass before committing: `uv run pytest`

## Common patterns

### Adding a new phase type
1. Add handler in `phases/your_type.py`
2. Dispatch in `runner.py:_run_phase()`
3. Add to `PhaseConfig.type` validation

### Adding a new tool
1. Implement in `tools/your_tool.py`
2. Add field to `ToolsConfig` in `models.py`
3. Add to `_build_tool_defs()` and `_dispatch_tool()` in `phases/agent.py`
4. Update `resolve_tools()` presets in `models.py` if needed

### Adding a new CLI command
Add to `flows/cli/main.py` (excluded from coverage). Test via integration or by testing the underlying module directly.

### Adding a new template
Drop `<name>.yml` in `flows/default_templates/`. It appears in `flows templates list` and `flows init` picker automatically.
