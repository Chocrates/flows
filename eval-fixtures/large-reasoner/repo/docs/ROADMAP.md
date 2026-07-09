# flows — Roadmap

## v0.2 — UX & Reliability

### 1. Streaming output ✓

**Problem:** foreground runs look frozen. LiteLLM blocks until full response; users see nothing for 30+ seconds on long phases.

**Solution:**
- Add `stream: bool = False` and `on_token: callable | None` params to `run_agent_phase`
- Accumulate streaming chunks (text + tool calls) with delta-merge logic
- Add `on_token(phase, token)` to `RunnerCallbacks`
- `ForegroundCallbacks.on_token` prints tokens immediately, clears status line on first token

**Files:** `flows/phases/agent.py`, `flows/runner.py`, `flows/cli/main.py`

---

### 2. `flows doctor` ✓

**Problem:** no way to validate config or test LLM connectivity before running a pipeline.

**Solution:**
- New CLI command `flows doctor [--config PATH] [--check-llm]`
- Shows: global config path (exists/not), resolved model + masked API key, project config (phases), env vars set, notifications status
- `--check-llm`: sends a 5-token test prompt, reports latency

**Files:** `flows/cli/main.py`

---

### 3. `flows artifacts` ✓

**Problem:** no way to inspect what phases produced without navigating `~/.flows/runs/` manually.

**Solution:**
- `flows artifacts [run_id]` — list all artifact files for a run with sizes
- `flows artifacts [run_id] [phase]` — pretty-print specific artifact as JSON

**Files:** `flows/cli/main.py`

---

### 4. `flows retry` ✓

**Problem:** failed or partial runs require starting over from scratch.

**Solution:**
- `flows retry [run_id] [--from phase]` — new run with same idea/config, start from specified or failed phase
- Creates a new run_id (clean state) re-using original idea, repo, template

**Files:** `flows/cli/main.py`

---

### 5. `--config` flag ✓

**Problem:** no way to point `flows run` at a non-default global config (testing, multi-workspace).

**Solution:**
- `--config PATH` option on `run`, `doctor`, `retry` commands
- Passed to `load_global_config(path)`

**Files:** `flows/cli/main.py`

---

### 6. Background worker env inheritance ✓

**Problem:** `subprocess.Popen` for background worker did not explicitly pass environment, potentially losing API keys set in the shell session.

**Solution:**
- Pass `env=os.environ.copy()` to `Popen` call in `_run_background`

**Files:** `flows/cli/main.py`

---

### 7. Cost guard (`max_cost_usd`) ✓

**Problem:** runaway pipelines with many phases can accumulate unexpected cost.

**Solution:**
- `max_cost_usd: float = 0.0` in `GlobalConfig` (0 = disabled)
- Configured in `~/.flows/config.yml` as `max_cost_usd: 5.00`
- Checked before each agent phase: reads accumulated cost from `llm.jsonl` via `parse_usage`, raises `RunError` with current spend if limit exceeded

**Files:** `flows/models.py`, `flows/config.py`, `flows/runner.py`

---

### 8. Improved prompt templates ✓

**Problem:** built-in prompts are minimal stubs — missing coding conventions, context usage patterns, structured output guidance.

**Solution:**
- `feature/plan.md`: add constraints checklist, edge case discovery, dependency analysis
- `feature/implement.md`: add read-before-write guidance, style conventions, completion criteria
- `research/*.md`: add source quality guidelines, gap analysis, citation format
- `software/*.md`: add phase-appropriate depth and handoff artifacts
- `review/review.md`: structured findings format

**Files:** `flows/prompts/**/*.md`

---

### 9. README ✓

**Problem:** no getting-started guide for the rebuilt tool.

**Solution:**
- Installation, config, first run (with Ollama and with Anthropic)
- Template overview with example `.flows.yml`
- CLI reference table
- Architecture diagram (ASCII)

**Files:** `README.md`

---

## v0.3 — Power Features (future)

- **`flows attach`** — reattach foreground UI to a background run
- **Token budget per phase** — `max_tokens: 4096` in `PhaseConfig`
- **Parallel phases** — `depends_on:` field for DAG execution
- **Container sandbox** — Docker-based execution for `agent` phases
- **Web UI** — `flows serve` for remote management
- **Pipeline registry** — share/import templates from GitHub
