# flows — design

Local-first workflow automation tool. Runs pipelines of heterogeneous phases (LLM agents, scripts, human gates) with a foreground-first interactive experience and optional background mode with desktop notifications.

---

## Mental model

A **pipeline** is a directed graph of **phases**. Each phase is one of:

- `agent` — LLM call with tool use (bash, read_file, write_file) running in-process against the project directory
- `script` — shell command, captures output, passes to next phase as context
- `human` — shows instructions and waits for user to signal completion

A **run** is one execution of a pipeline against a specific idea/input. Runs are durable on disk — you can detach and reattach, or pick up after a crash.

---

## Interaction model

### Home screen: `flows` (no args)

Running `flows` with no subcommand and a TTY launches the launcher — a
ratatui home screen listing recent runs (auto-refreshes every 3s) with
bindings `n` new run, `r`/`enter` resume/retry a run, `q` quit. Starting a new
run opens a new-run form (idea input + template picker); selecting
a run opens the attach view. Both actions hand off into the same `App`
used by `flows run`. With no TTY, prints `--help` instead.

### Default: TUI

When stdout is a TTY, `flows run` launches a full-screen ratatui TUI (`App`):
- Left sidebar: live phase status (`○` pending · `⟳` running · `✓` done · `✗` failed)
- Right pane: streaming LLM output, tool calls, script results
- Bottom bar: input for human gates and clarification questions

### Foreground plain

```bash
flows run "idea"          # plain foreground (add --tui for the full-screen view)
```

Output printed to terminal. Questions appear inline, answered inline.

### Background mode (long-running tasks)

```bash
flows run "idea" --background     # detach immediately, prints run ID
flows runs                        # list runs + status + cost
flows attach <run_id>             # reattach: live output, answer questions,
                                  #   approve gates, cancel — all interactively
flows usage <run_id>              # per-phase tokens + cost
```

The launcher (`flows` with no args) spawns a background worker and attaches to
it, so a new run and a reattach are the same view. Answers/approvals/cancels are
written as signal files that the worker polls. A desktop notification fires when
a backgrounded run needs human input.

---

## Pipeline templates

`flows init` picks a template. Built-in templates:

### `feature` — local feature work
```
plan → implement → lint → test → review↩ → approve↩ → commit
```
`review` is an agent phase: it reads lint/test output, fixes issues in place, then
loops back to `lint` (re-running lint → test → review) until the verdict is
`approved`/`approved_with_suggestions` or `max_loop_iterations` (3) is hit.
`approve` is a human gate that can loop back to `implement`.

### `software` — full software project
```
requirements → spec → design → implement → lint → test → review↩ → approve↩ → commit
```
Same agent-review-and-fix loop as `feature` (review → lint), plus a human `approve` gate.

### `research` — research and synthesis
```
gather → synthesize↩ → report → approve↩
```

### `review` — code/doc review
```
review → approve↩
```
Agent reviews, then a human `approve` gate loops back to `review` for a deeper look.

### `blank` — single agent
```
run
```

↩ marks phases with `loop_back_to` configured.

Templates are embedded in the binary from `crates/flows-core/assets/default_templates/*.yml`. Lookup order (first match wins):
`<repo>/.flows/templates/<name>.yml` (project) → `~/.flows/templates/<name>.yml`
(global user) → builtin. `flows init` copies the chosen template into the project
`.flows/` for editing; `flows init --global` copies all templates into `~/.flows/`.

---

## Pipeline config (.flows.yml)

```yaml
llm:
  model: claude-sonnet-4-6
  api_key: ${ANTHROPIC_API_KEY}

phases:
  - name: plan
    type: agent
    tools: files                    # preset
    max_clarification_rounds: 3

  - name: implement
    type: agent
    tools:
      files: true
      bash:
        allowed: ["make*", "cargo*", "npm*", "go*"]
    on_failure: stop

  - name: lint
    type: script
    command: "@auto:lint"           # resolved in Rust: make lint → cargo clippy / ruff / …
    on_failure: retry:implement     # stop | continue | retry:<phase>

  - name: test
    type: script
    command: "@auto:test"           # make test → cargo test / pytest / npm run test / go test
    on_failure: retry:implement     # (an unresolvable tool is a hard stop, regardless)

  - name: review
    type: agent                     # reads lint/test artifacts, fixes issues, verifies
    tools: files_and_test
    loop_back_to: implement         # re-run implement → lint → test → review after fixes
    loop_condition: "content.get('verdict') not in ('approved', 'approved_with_suggestions')"
    max_loop_iterations: 3

  - name: approve
    type: human
    instructions: "Review the diff. Type feedback or Enter to accept."
    loop_back_to: implement
    loop_condition: "content.get('answer', '').strip() not in ('', 'ok', 'approve')"
    max_loop_iterations: 5

  - name: commit
    type: script
    command: "git add -A && git commit -m 'feat: {{ idea[:60] }}'"
    on_failure: continue

context:
  exclude:
    - "*.lock"
    - "target/"
    - "node_modules/"
```

### DAG (parallel execution)

Set `depends_on` on phases to run them in parallel:

```yaml
phases:
  - name: implement
    type: agent
    tools: files

  - name: lint
    type: script
    command: "@auto:lint"
    depends_on: [implement]        # runs after implement

  - name: test
    type: script
    command: "@auto:test"
    depends_on: [implement]        # runs in parallel with lint

  - name: review
    type: human
    depends_on: [lint, test]       # waits for both
```

**DAG restrictions**: `loop_back_to` and `retry:X` on_failure cannot be used in DAG mode. Human phases in DAG mode are serialized (wait for all in-flight tasks before running).

---

## Config system (3-tier)

Lowest to highest priority:

```
~/.flows/config.yml               # global user defaults
~/.flows/projects/<repo-name>.yml # user project override (not committed)
.flows.yml                        # repo project config (committed)
```

LLM config: project wins over user-project-override wins over global.  
`max_cost_usd` and `notifications` live in global/user-project layers only.

### Global config (~/.flows/config.yml)
```yaml
llm:
  model: claude-sonnet-4-6
  api_key: ${ANTHROPIC_API_KEY}
  thinking: true              # extended thinking/reasoning; on by default, set false to disable
  thinking_budget_tokens: 2048 # only used when thinking is enabled

max_cost_usd: 5.00

notifications:
  enabled: true

tools:
  web_search:
    backend: duckduckgo   # duckduckgo | brave | serper
    max_results: 5
```

### Extended thinking / reasoning

`thinking` and `thinking_budget_tokens` are tri-state at every config layer
(`None` = inherit from the next layer down; only a concrete `true`/`false`
overrides). Precedence: phase (`PhaseConfig.thinking`) > project `.flows.yml`
`llm.thinking` > user-project-override > global `~/.flows/config.yml` >
hardcoded default (enabled, 2048 budget tokens). Resolved once per agent call
in `flows-runner::agent::resolve_thinking`.

Only applied when the resolved model supports reasoning — unsupported models silently skip the `thinking` param
instead of erroring. When enabled, `max_tokens` is set to
`budget_tokens + 4096` (Anthropic requires `max_tokens > budget_tokens`).
Reasoning content streams via a separate `on_thinking` callback (kept apart
from `on_token`/the regular content stream); the TUI renders it dimmed/italic,
distinct from the final answer. `flows estimate`/`--dry-run` add the thinking
budget to the output-token estimate for phases where it's active, since
thinking tokens are billed as output tokens.

Disable per-phase (e.g. a cheap fix-loop phase that doesn't need deep
reasoning): add `thinking: false` under that phase in the template YAML.
On OpenAI-compatible endpoints other than `api.openai.com`, an explicit
`thinking: false` is sent as `chat_template_kwargs: {enable_thinking: false}`
(honoured by Qwen-style chat templates; unknown kwargs are ignored by others).
Budgets and `reasoning_effort` are not sent there — the probed vLLM server
ignored both.

---

## Runner architecture

`run_pipeline()` in `runner.rs` is the core execution engine:

1. Validates pipeline config (`flows_core::pipeline::validate_pipeline`, including `when` syntax). Pipelines arrive flat: `include:` entries were expanded at load time (`flows_core::include`), with each included phase's `origin` (template, local name, alias prefix, `with:` vars) used for prompt lookup, `vars`, and short-name artifact views
2. Writes `RunMeta` and `RunState` to `~/.flows/runs/{run_id}/`
3. Merges LLM config (global → project) and resolves per-phase model profiles
4. Detects execution mode:
   - **Linear**: no phase has `depends_on` → `run_linear()` (sequential, supports loops + retry)
   - **DAG**: any phase has `depends_on` → `run_dag()` (ready phases run concurrently on a `FuturesUnordered`)
5. Before each phase, evaluates its `when` condition (`flows_core::condition`); a false
   condition records a `{skipped: true}` artifact and fires `on_phase_skipped`
6. Calls `execute_phase()` for individual phase execution (agent / script / human)
7. Fires `RunnerCallbacks` events throughout

All UI (TUI, foreground print, background notify) is implemented via `RunnerCallbacks`. Core modules never print to stdout.

---

## Phase execution

### Agent phase (flows-runner::agent)

1. Load project context (file tree, respecting `.gitignore` + exclude)
2. Load prior phase artifacts as context (injected **once** as the `prior_artifacts` map)
3. Assemble optional context-delivery features (all gated by the `context:` block, default off)
4. Render prompt template (MiniJinja) with: `idea`, `repo_tree`, `repo_map`, `pushed_context`, `tools_enabled`, `prior_artifacts`
5. Resolve extended thinking and run the provider completion with a tool loop (tools enabled per phase config; a `supports_tools: false` model gets no schemas and no tool prompt sections)
6. Clarification loop: up to `max_clarification_rounds`, then proceed with best guess
7. Loop guards end a phase that has stopped making progress, each with a note in the output pane:
   - identical failing calls (`MAX_REPEATED_TOOL_ERRORS`), unchanged rewrites of one file (nudge at 3, abort at 6), 30 rounds without a write once writing has started, and the round cap;
   - **empty replies**: nudged, the phase fails after 3;
   - **output cap**: a reply stopped by `max_tokens` (`length` / `max_tokens`) with no tool call is nudged to be brief rather than taken as the artifact; a second in a row fails the phase naming the limit;
   - **stalls**: rounds whose calls bring nothing new (deduplicated re-reads, identical repeats of a read-only call, dead or empty searches, failed or 4xx/5xx fetches) nudge at 3; at 5 a read-only phase gets `tool_choice: none` and must reply; a write-capable phase aborts at 8;
   - **exploration without novelty**: rounds of reads / listings / searches that surface no unseen path and no new read nudge at 6 and force a read-only phase to reply at 10;
   - **dead search**: a `web_search` that cannot reach its backend (or 6 empty results) turns search off for the phase and drops it from the offered tools.

#### Context delivery (small models)

Optional, independently flagged, default-off features that fit a large repo into a
small window — some **pull** (a tool the model calls), some **push** (injected):

- **`search_code`** (pull) — repo-scoped ripgrep-style search (`flows-tools::code_search`).
- **range reads** (pull) — `read_file(path, start_line, end_line)` reads slices, not whole files.
- **slim tree** (push) — bounded, gitignore-aware repo tree (`repo_context::build_slim_tree`).
- **repo map** (push) — tree-sitter symbol signatures per file, ranked by task overlap with a git-recency tie-breaker (git runs inside the sandbox, since agents can write `.git/config`; mtime fallback), token-bounded (`repo_map`).
- **budgeting** — before each send, trim oldest tool-result bodies once estimated tokens exceed `budget_fraction × window` (`budget`), preserving system prompt + task + recent turns.
- **retrieval** (push) — local in-process embeddings (`fastembed`/ONNX) of **symbol-aware chunks** (`flows-runner::chunker`: tree-sitter top-level nodes, comments attached to the next definition, oversized nodes split into members with the enclosing header as embedding context, small neighbours merged; line windows for other languages). Each chunk is embedded with its path. Chunks are ranked by reciprocal-rank fusion of cosine similarity and BM25 (path + text, camel/snake tokens, light stemming) to the task and packed best-first into `retrieval_budget_fraction` (0.25) of the phase model's window, capped at `retrieval_max_chunks` (`retrieval`). The index (≤ 1500 chunks, code before docs/config, truncation reported) is synced once per flow and persisted at `~/.flows/index/<repo-hash>/<model>.json`. Vectors are content-addressed (blake3 of the embedded text), so only new/changed chunks are embedded; vectors of vanished chunks stay in a spare pool (≤ 1500) for branch switches; a model or chunker-version change re-embeds all. The index dir is capped at 1 GiB: orphaned repos first, then LRU (load touches mtime), never the current repo. The embedding model downloads to `~/.flows/models` (`FASTEMBED_CACHE_DIR` overrides). Embedding goes through a per-model shared vector cache (`~/.flows/vectors/<model>.bin`, append-only `[key][dim][f32…]` records, compacted to the newest half past 256 MiB), so identical chunks at another path are never re-embedded.

Push features (repo map, retrieval) and tool-gating make the agent usable by
reasoning models that can't function-call. Feature usefulness is meant to be a
**measured** delta — see `flows eval` / `flows eval retrieval`.

### Script phase (flows-runner::script)

1. Render command template (MiniJinja, `{{ idea }}` available)
2. Run via subprocess in repo root
3. Capture stdout/stderr, exit code
4. Produce artifact `{"stdout": ..., "stderr": ..., "exit_code": ...}`
5. Apply `on_failure` policy

A non-zero exit raises `ScriptFailed`, which carries the artifact. The runner
**always** stores that artifact in `prior_artifacts` (and writes it to disk)
before applying `on_failure` — so a downstream `review` agent can read lint/test
output even when the command failed. This holds in both linear and DAG modes.

### Human phase (flows-runner::runner)

1. Display `instructions` via callback
2. Await response (foreground: an inline prompt; background: poll signal files; TUI: a channel)
3. Produce artifact with `{"answer": response_text}`

---

## Tool system

Tools are enabled per phase via `ToolsConfig`. Presets: `files`, `files_and_test`, `files_and_deps`, `research`, `research_and_code`, `all`.

Plugin tools: `~/.flows/tools/<name>.yml` define custom LLM tools with a MiniJinja shell command template.

### Command sandboxing (flows-tools::sandbox)

Every tool that runs a shell command — the `bash` tool and plugin tools — executes through `run_confined`, which wraps the command in **bubblewrap** (`bwrap`): the whole filesystem is bound **read-only** except the project directory (**read-write**) and a fresh private `/tmp`. So a command can read system libraries/config (tools work) but cannot modify, delete, or create anything outside the project tree, and `cd` out is harmless because everything out there is read-only.

This is the *actual* containment boundary. An allowlist alone cannot confine a shell command: once an interpreter (`python`, `make`, `npm`, …) is permitted it can touch anything the process can reach, and `cwd=repo_root` does nothing against an absolute path or a `..`. (Demonstrated: with only `python*` allowed, `python3 -c "open('/tmp/pwn','w')…"` wrote outside the repo.) The allowlist restricts *which* programs may run; the sandbox restricts *where* they may act — both are needed.

- **No host IPC:** a read-only bind doesn't stop `connect()` on unix sockets, and the user's session bus alone is a full escape (`systemd-run --user` runs commands outside the sandbox). bwrap therefore gets a seccomp filter (`sandbox/seccomp.rs`, hand-assembled BPF handed over via `--seccomp FD`) that denies `socket(AF_UNIX)`, `io_uring_setup`, and foreign-ABI syscalls. `socketpair()` and inet sockets stay allowed, so builds, dependency fetches, and in-process IPC work. macOS doesn't have this yet (see ROADMAP).
- **Resource limits:** each command tree runs in a transient `systemd-run --user --scope` with `MemoryMax` (default 8 GB, no swap) and `TasksMax` (1024), overridable per phase (`tools.bash.memory_mb` / `max_procs`). A probe reads the limits back from the scope's cgroup, so hosts that don't delegate the memory/pids controllers aren't mistaken for limited. Without a scope: per-process `RLIMIT_DATA` (Linux) or nothing (macOS). Limits never block a command; a phase note and `flows doctor` report the weaker mode. `systemd-run` runs *outside* bwrap with only the bus env it needs; bwrap clears it and seccomp blocks the bus for the command.
- **Fails closed:** if `bwrap` (with unprivileged user namespaces) isn't available, shell commands refuse to run rather than run unconfined. Probe result is cached (`sandbox_available`).
- **Network** is left shared so package/build commands can fetch dependencies; only filesystem writes are confined.
- Real end-to-end containment is verified by `test_sandbox_confines_writes_to_repo`, which is `skipif`-gated on a functional `bwrap` (nested sandboxes that block user namespaces skip it; a normal Linux host runs it). Unit tests cover the argv, the availability probe, fail-closed behaviour, and output handling by mocking the sandbox seam.

### Bash restrictions

- Blocked by default; opt-in with explicit allowlist (the `implement` phase in `feature.yml` allows `make*`/`python*`/`uv*`/`npm*`/`rm*` — `rm` is safe to grant because the sandbox confines it to the project directory).
- Hardcoded blocks regardless of config (defense in depth, checked before the sandbox): `rm -rf /`, `curl|bash`, `sudo`, writes to `/etc/`, etc.

### File tool scoping

`read_file`/`write_file`/`list_files` resolve the path under repo root (`_safe_path`: `(repo_root / user_path).resolve()` then `is_relative_to(repo_root.resolve())`), so `../` traversal, absolute paths (`repo_root / "/etc/passwd"` → `/etc/passwd` → rejected), and symlink escapes are all blocked.

### `http_get` restrictions

GET only; `http`/`https` schemes only (no `file://`); blocks private/loopback/link-local IPs; 10s timeout; 100KB max response. **Known gap:** redirects are followed without re-checking the target against the private-IP blocklist, so a public URL that 3xx-redirects to `169.254.169.254` (cloud metadata) can bypass it — a network SSRF concern, separate from filesystem scoping, not yet fixed.

---

## Prompt loading

Lookup order for phase `implement` in template `feature` (first match wins):

1. `phase.prompt` field (explicit path or key)
2. `<repo>/.flows/prompts/feature/implement.md` (project override)
3. `~/.flows/prompts/feature/implement.md` (global user override)
4. builtin `crates/flows-runner/assets/prompts/feature/implement.md` (embedded)
5. `<repo>/.flows/prompts/implement.md` (project generic)
6. `~/.flows/prompts/implement.md` (global user generic)
7. builtin generic `crates/flows-runner/assets/prompts/implement.md` (embedded)
8. Auto-generated fallback

`flows init` copies the chosen template's prompts into `<repo>/.flows/prompts/<template>/`
so a project can customize its agents; global overrides live in `~/.flows/prompts/`
(seeded by `flows init --global` or `flows prompts copy feature implement`).

---

## State on disk

```
~/.flows/
  config.yml
  projects/
    <repo-name>.yml   ← user project overrides (not committed)
  templates/
    <name>.yml        ← user template shadows
  prompts/
    <template>/
      <phase>.md      ← user prompt overrides
  tools/
    <name>.yml        ← plugin tool definitions
  runs/
    {run_id}/
      meta.json       ← idea, repo_path, template, phases, created_at, pid
      state.json      ← status, current_phase, completed_phases, questions, error
      llm.jsonl       ← every LLM call (request, response, tool calls, usage)
      artifacts/
        {phase}.yml   ← each phase's output
      signals/
        cancel        ← touch to cancel
        advance       ← touch to advance human gate
        answer.txt    ← answer text for background Q&A
      worker.log      ← stdout from background worker
```

State files written atomically (temp file + rename). Signal files polled by background worker.

### Project-level `.flows/` (in the repo)

`flows init` seeds a per-project directory (committable) that shadows the global tiers:

```
<repo>/
  .flows.yml            ← repo project config (committed)
  .flows/
    templates/
      <name>.yml        ← project template override (copied from builtin on init)
    prompts/
      <template>/
        <phase>.md      ← project prompt override (copied from builtin on init)
```

Runs always live under `~/.flows/runs/`, never in the project `.flows/`.

---

## TUI (flows-tui)

ratatui + crossterm, immediate mode: every frame is drawn from plain model
structs (`App`, `OutputPane`, `PhasePanel`), so behaviour lives in methods that
are unit-tested without a terminal, and `render.rs` draws into a `Buffer`.

### Screens and data flow

- **Home** (`home.rs`, `flows` with no args): recent runs plus "＋ New run"
  (idea + template form). `n` new, `Enter` open, `d` delete, `q`/`Esc` quit.
  Starting a run spawns a background worker and attaches to it.
- **Run view** = **attach view** (`attach.rs`). `flows run --tui` and the home
  screen both start a detached worker and attach, so `q` only detaches; the run
  keeps going (`flows attach <id>` / `flows stop <id>`). The view is fed by
  polling the run's files every 200 ms: `state.json` → phase status, current
  phase, finished/failed/cancelled; `output.log` → output lines (tailed from a
  byte offset); `llm.jsonl` → cost; `meta.json` → phase names and loop edges.
- **In-process run** (`run.rs::run_tui`): the pipeline on a tokio task, events
  over an `mpsc` channel via `ChannelCallbacks` (`RunnerCallbacks` →
  `UiEvent`). Questions and human gates send `UiEvent::NeedInput` with a
  `oneshot` reply. The CLI uses the worker path; this one remains for embedding
  and tests.

`output.log` is categorized JSONL, `{"c": <cat>, "x": <text>}`, written by both
`ChannelCallbacks` (via `OutputLog`) and the background worker, so a run is
replayable from the start on any attach. Categories: `n` normal, `t` thinking,
`j` JSON, `o` tool, `s` system prompt, `u` user, `m` meta, plus metric records
(`ctx`, `cost`, `cw`).

When attached, answers go back as signal files: the chat bar writes
`signals/answer.txt`; `c`/`Esc` write a cancel signal. Stale signals are cleared
on attach to a finished run so they can't be misread later.

### Output pane (`output.rs`)

`OutputPane` stores logical lines tagged with a `Category` (`Normal`,
`Thinking`, `Json`, `Tool`, `System`, `Meta`). `push_token` assembles streamed
tokens into the current line and commits on `\n`; a category change commits the
partial line first. `visible_entries()` filters by toggles and word-wraps to the
viewport width (hard-breaking long words), and scrolling, selection and
rendering all work on that same wrapped list, so they always agree.

- **Toggles** filter at view time, so they hide or show content already on
  screen: `t` thinking, `j` the JSON artifact block, `s` the system prompt (off by
  default). `Meta` (phase markers, the task, script results) is never hidden.
- **What gets tagged where** is decided upstream: the agent's `StreamFilter`
  buffers the start of each response and drops a `questions` envelope entirely
  (raw clarification JSON never reaches the pane), and its fence splitter tags
  the fenced artifact block as `TokenKind::Json` → `on_json`. Tool calls arrive
  as one-line summaries (`summarize_tool_call` / `summarize_tool_result`);
  errors render red.
- **Sticky follow.** `follow` is a flag, not a recomputed position: scrolling
  up clears it, reaching the bottom (scroll down, `End`) sets it, and new content
  scrolls to the bottom only while it is set.
- **A selection suspends follow.** While a non-empty selection exists, new
  output does not scroll, so streaming can't drag the highlighted text away. A
  plain click (zero-width selection) does not suspend it.

### Selection and copy

Mouse capture is on, so the terminal's own selection is unavailable; the app
does it. Left-drag inside the output pane selects (cells in wrapped-line
coordinates, painted with `SELECTION_BG`); the wheel scrolls 3 lines.
`Ctrl+C` copies the selection, `y` copies the whole visible buffer. Both go
through `arboard` (system clipboard) and report `copied N chars` or the error in
the status line. `Ctrl+C` never quits; `q` detaches.

### Phases pane (`phases.rs`)

Status glyphs: `○` pending, `▶` running, `↻` looping, `✓` done, `✗` failed,
`⊘` skipped. Loop edges (`loop_back_to`) are drawn as brackets in a gutter from
target to source, highlighted while active, and a loop source shows its
iteration budget (`review ↺1/3`). On a loop-back, `restart_range` resets every
phase from the target through the source to looping, since the whole segment
re-runs. On attach, `mark_recorded_phases` rebuilds statuses (done, skipped,
current) from `state.json`.

### Input and status

While the run waits for an answer, a question panel opens above the status line
(several questions are numbered one per line); typing goes to it, and output
navigation moves to `Alt+t/j/s` plus arrows/PgUp/PgDn. The status line shows
phase progress, tool round `N/T`, context-window usage and cost.

### Tests

Model behaviour (follow, selection, toggles, wrapping, loop spans, key
handling) is unit-tested in `tests/{output,phases,app,render,home}.rs`.
`tests/pty.rs` runs the `tui_demo` binary under a real PTY (`portable-pty`),
feeds its output to a `vt100` emulator, sends real SGR mouse sequences, and
asserts on rendered cells, including that the selection background reaches the
screen. That catches what `TestBackend` can't: real mouse decoding and actual
colours.

---

## Template registry (flows-core::registry)

`flows templates install <git-url>[//path][@ref]` clones with hooks disabled
(`core.hooksPath=/dev/null`, `protocol.ext.allow=never`, no terminal prompts),
resolves the ref to a commit, and reads one template plus `prompts/<stem>/*.md`
(regular files only, no symlinks, ≤ 256 KiB each). A review (phases, script
commands, bash settings, includes, prompts) is printed before anything is
written. Installs go to `~/.flows/templates/<name>.yml` +
`~/.flows/prompts/<name>/`, recorded in `~/.flows/templates/registry.lock`
(source, path, ref, commit, file hashes, trust).

Trust is enforced at load: `registry::mark_untrusted` marks every phase whose
defining template (the loaded one, or an `include:` origin) resolved to an
untrusted install — global copy wins the lookup and the lockfile lists it
without trust. Marked phases get `PhaseConfig.untrusted` (serialized into run
meta) and `expose_credentials` cleared; `script.rs` runs their commands through
`sandbox::confined_command` (same wrapping, limits and env scrubbing as the bash
tool) instead of `/bin/sh` on the host. `update` resets trust unless re-granted.

## Cost estimation (flows-core::estimate)

`estimate_pipeline(phases, idea, template_name, repo_root, llm)` returns `list[PhaseEstimate]`.

- For `script` and `human` phases: cost = $0
- For `agent` phases:
  - Renders prompt with stub context (no actual repo files)
  - `tiktoken` counts input tokens for OpenAI-family models (chars/4 heuristic otherwise)
  - Heuristic output token count by phase name (`plan`=600, `implement`=2000, ...)
  - Cost range: `[in_tok * input_price + out_low * output_price, in_tok * input_price + out_high * output_price]`

No LLM calls made during estimation.

---

## Dry run (flows-core::dryrun)

`dry_run(idea, phases, template_name, global_cfg, project_cfg, repo_root)` returns a dict:

- `errors`: validation errors (depends_on targets exist, no loops in DAG, model set, ...)
- `plan_lines`: human-readable execution plan (linear numbered list or DAG with deps)
- `estimates`: per-phase cost estimates
- `total_cost_low / total_cost_high`: summed estimates

CLI: `flows run "idea" --dry-run` or `flows estimate "idea"`.

---

## CLI commands

```bash
# Home screen — recent runs, start new work, resume/retry
flows                                    # TUI launcher when TTY, --help otherwise

# Run a pipeline
flows run "idea"                         # TUI when TTY, plain otherwise
flows run "idea"          # plain foreground (add --tui for the full-screen view)                # force plain output
flows run "idea" --background            # detach; desktop notification on completion
flows run "idea" --dry-run               # validate + show plan + estimate; no LLM calls
flows run "idea" --template feature

# Cost estimate
flows estimate "idea"                    # same as --dry-run but standalone command

# Init
flows init                               # pick template; write .flows.yml + copy template/prompts into .flows/
flows init --template research
flows init --generate                    # detect stack, generate tailored config
flows init --global                      # write ~/.flows/config.yml + copy all templates/prompts into ~/.flows/

# List and inspect runs
flows list                               # recent runs with status
flows status [run_id]                    # current phase and state
flows artifacts [run_id]                 # list phase artifacts
flows artifacts [run_id] <phase>         # show artifact content

# Inspect logs
flows logs <run_id>                      # phase-grouped summary
flows logs <run_id> --phase implement    # one phase only
flows logs <run_id> --full               # full message content + tool I/O
flows logs <run_id> --raw                # raw JSONL
flows usage [run_id]                     # token counts + cost breakdown

# Control background runs
flows answer <run_id> "my answer"        # answer a waiting question
flows advance <run_id>                   # approve a human gate
flows cancel <run_id>                    # cancel a running pipeline

# Retry
flows retry <run_id>                     # retry from last failed phase
flows retry <run_id> --from plan         # retry from a specific phase

# Diagnostics
flows doctor                             # check config, API keys, env vars, required packages
flows doctor --check-llm                 # also send a test LLM request

# Templates
flows templates list                     # list all templates (project → global → builtin)
flows templates sync [--force]           # copy builtin templates/prompts into ~/.flows/
flows templates new <name>               # scaffold a commented template
flows templates install <git>[//path][@ref] [--as N] [--trust] [--yes]
flows templates installed | update <name> | remove <name> | trust <name> [--revoke]

# Prompts
flows prompts list                       # list all agent prompts
flows prompts list --template feature    # filter by template
flows prompts show <template> <phase>    # print prompt markdown
flows prompts copy <template> <phase>    # copy to ~/.flows/prompts/ for editing

# Plugin tools
flows plugins list                       # list user-defined tools in ~/.flows/tools/
flows plugins show <name>                # print plugin tool YAML
```

---

## LLM interaction logging

Every LLM interaction is logged to `~/.flows/runs/{run_id}/llm.jsonl`.

```jsonl
{"ts":"...","event":"phase_start","phase":"plan","run_id":"a3f2"}
{"ts":"...","event":"request","phase":"plan","round":0,"model":"claude-sonnet-4-6","messages":[...],"tools":["read_file"]}
{"ts":"...","event":"response","phase":"plan","round":0,"content":"...","tool_calls":[...],"usage":{"input_tokens":1240,"output_tokens":380},"latency_ms":14200,"cost_usd":0.011}
{"ts":"...","event":"tool_call","phase":"plan","tool":"read_file","args":{"path":"README.md"},"duration_ms":3}
{"ts":"...","event":"tool_result","phase":"plan","tool":"read_file","result_bytes":4210,"truncated":false}
{"ts":"...","event":"phase_end","phase":"plan","artifact_written":true,"total_input_tokens":3340,"total_output_tokens":590,"cost_usd":0.031}
{"ts":"...","event":"error","phase":"plan","error":"RateLimitError","message":"..."}
```

---

## Implementation (Rust workspace)

Six crates:

- **`flows-core`** — `models`, `config` (3-tier merge, env expansion), `state` (atomic R/W, signal files), `templates` (embedded builtins + override lookup), `pricing`, `usage` (parse `llm.jsonl`), `estimate` (tiktoken), `detect`, `dryrun`, `notify`, `llm_log`, `thinking`.
- **`flows-tools`** — `files` (repo-scoped, `O_NOFOLLOW`), `sandbox` (backend trait → **bwrap** on Linux, **Seatbelt** on macOS; env-cleared; fail-closed), `bash` (allowlist over the sandbox), `plugin`, `http` (SSRF guard + redirect revalidation), `search` (DuckDuckGo).
- **`flows-llm`** — `Provider` trait; Anthropic Messages + OpenAI Chat Completions translation & streaming SSE (incl. `reasoning_content`); `make_provider` selects by model name.
- **`flows-runner`** — `agent` (LLM tool loop, `StreamFilter`, thinking, mid-phase cancel), `script` (+ `@auto:test`/`@auto:lint` resolver), `prompts` (MiniJinja + `AGENTS.md`), `repo_context`, `runner` (linear + DAG, loops, retries, cost guard).
- **`flows-tui`** — ratatui immediate-mode: `App` (shared by live run + reattach), `OutputPane` (sticky scroll, selection, category styling + toggles), `PhasePanel` (loop brackets), the launcher home screen, the attach view (polls state/output.log/llm.jsonl).
- **`flows-cli`** — the `flows` binary: `doctor`, `config`, `completions`, `init`, `templates`, `plan`, `run [--tui|--background]`, `runs`, `usage`, `attach`, `worker` (hidden).

Cross-cutting: background worker + signal files (answer/advance/cancel), desktop notifications, live token streaming to `output.log`, per-phase token/cost accounting, CI (fmt + clippy `-D warnings` + tests).

---

## What's next

- **Pipeline composition**: `include` another pipeline as a phase.
- **Template registry**: community-shared templates.

---

## Dependencies (key crates)

```toml
serde / serde_json / serde_yaml_ng   # config, state, artifacts (byte-compatible on disk)
tokio + reqwest (rustls)             # async runtime + HTTP (LLM providers, http_get)
minijinja + minijinja-contrib        # prompt templates + loop conditions (Jinja/pycompat)
ratatui + crossterm                  # TUI; arboard clipboard; portable-pty + vt100 (PTY tests)
include_dir                          # embed builtin templates + prompts in the binary
tiktoken-rs                          # token counting for OpenAI-family estimates
ignore                               # gitignore-aware repo tree
clap + clap_complete                 # CLI + shell completions
libc                                 # O_NOFOLLOW; bubblewrap / sandbox-exec are external
```

No Temporal. No k8s. No Docker. Shell sandbox = bubblewrap (Linux) / Seatbelt (macOS).
