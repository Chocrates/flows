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

---

## Runner architecture

`run_pipeline()` in `runner.rs` is the core execution engine:

1. Validates pipeline config (`_validate_pipeline`)
2. Writes `RunMeta` and `RunState` to `~/.flows/runs/{run_id}/`
3. Merges LLM config (global → project)
4. Detects execution mode:
   - **Linear**: no phase has `depends_on` → `_run_linear()` (sequential, supports loops + retry)
   - **DAG**: any phase has `depends_on` → `_run_dag()` (parallel, `asyncio.wait(FIRST_COMPLETED)`)
5. Calls `_run_phase()` for individual phase execution (agent / script / human)
6. Fires `RunnerCallbacks` events throughout

All UI (TUI, foreground print, background notify) is implemented via `RunnerCallbacks`. Core modules never print to stdout.

---

## Phase execution

### Agent phase (flows-runner::agent)

1. Load project context (file tree + file contents, respecting `.gitignore` + exclude)
2. Load prior phase artifacts as context
3. Render prompt template (MiniJinja) with: `idea`, `repo_tree`, `repo_files`, prior artifact vars
4. Resolve extended thinking (see below) and run the provider (Anthropic/OpenAI) completion with a tool loop (tools enabled per phase config)
5. Clarification loop: up to `max_clarification_rounds`, then proceed with best guess

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
4. `flows/prompts/feature/implement.md` (builtin)
5. `<repo>/.flows/prompts/implement.md` (project generic)
6. `~/.flows/prompts/implement.md` (global user generic)
7. `flows/prompts/implement.md` (builtin generic)
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

ratatui immediate-mode full-screen terminal application. Architecture:

- `App` — the shared TUI model, used by both a live run and a reattach
- `ChannelCallbacks(RunnerCallbacks)` — bridges runner events to `UiEvent`s over a channel
- `PhasePanel(Static)` — left sidebar with live phase status icons
- the pipeline runs on a tokio task; a foreground run streams events over a channel, a reattach polls the run's files (state.json / output.log / llm.jsonl)
- Human input: `asyncio.Event` — worker awaits it; app's `on_input_submitted` sets it

Token streaming: buffered in `_token_buf`, split on `\n`, complete lines written to `RichLog`. Partial trailing line held until next newline or phase boundary. All dynamic content is `rich.markup.escape()`d before writing.

Structured-response handling: `StreamFilter` (flows-runner::agent) buffers the first ~30 chars of a streamed response; if it begins with a `questions` JSON envelope, those tokens are dropped from `on_token` so raw clarification JSON never reaches the output pane. Artifact JSON is still streamed.

Thinking/reasoning streaming: a separate `_thinking_buf` + `on_flow_thinking` handler renders extended-thinking deltas dimmed/italic, prefixed with a one-time `thinking…` marker, kept out of the regular content buffer so it can't be mistaken for the actual answer.

Toggleable thinking / JSON output: every line written to the output pane is recorded in `self._entries` as `(category, markup)`, where category is `thinking`, `json`, or `normal`. Streamed model tokens are classified line-by-line — a ```` ```json ```` fence flips `_in_json_fence` so the artifact envelope (fenced per the prompt templates) is tagged `json`; everything else is `normal`. `t` toggles thinking, `j` toggles JSON. Toggling flips `_show_thinking`/`_show_json` and calls `_rerender_output()`, which `clear()`s the log and replays only the currently-visible entries — so a toggle hides/shows **already-displayed** content, not just future output. `_in_json_fence` resets on each `FlowPhaseStarted` so an unterminated fence in one phase can't swallow the next phase's prose.

Tool call visibility: `summarize_tool_call`/`summarize_tool_result` (flows-runner::agent) build one-line, tool-specific summaries (e.g. `read_file "foo.rs"`, `bash "cargo test"`, `write_file "out.rs" (240 chars)`) instead of a bare tool name — surfaced via dedicated `on_tool_call`/`on_tool_result` callbacks (not the generic `on_status`, which they replaced for this purpose) in every callback implementation (TUI, foreground plain, background worker). Results longer than ~150 chars are truncated to a first-line preview + total length; errors (`result.starts_with("Error:")`) render in red.

Messages: `FlowPhaseStarted`, `FlowPhaseEnded`, `FlowToken`, `FlowThinking`, `FlowToolCall`, `FlowToolResult`, `FlowStatus`, `FlowScriptResult`, `FlowLoopBack`, `FlowInputRequired`, `FlowRunComplete`, `FlowRunFailed`.

### Loop visibility (`PhasePanel`)

A phase resuming from a `loop_back_to` used to just get reset to the generic `pending` status — visually identical to a phase that had never run, with no indication a loop was happening or how many iterations occurred. `PhasePanel` now has:

- A distinct `looping` icon/color (`↻` magenta) — separate from `pending` (`○` dim) and `running` (`⟳` yellow) — so a repeating phase reads differently from a fresh one.
- `mark_looping_range(from_phase, to_phase)`: flags **every** phase between `to_phase` and `from_phase` inclusive as `looping`, not just the tail phase that decided to loop — since a loop-back re-executes that whole segment (e.g. `review → lint` re-runs `lint`, `test`, and `review`). Naturally overwritten back to `running`/`done` as each phase actually restarts via the normal `on_phase_start`/`on_phase_end` flow.
- `set_loop_iteration(phase, iteration, max_iterations)`: a persistent `(i/max)` counter rendered next to the phase name, so the sidebar keeps showing how many times a phase looped even after the run moves on.

### Mouse-select + copy (`OutputPane`)

Plain `RichLog` has three separate gaps versus ratatui's built-in text-selection system (click-drag highlight, Ctrl+C → `Screen.action_copy_text`, OSC 52 clipboard):

1. It doesn't implement `Widget.get_selection()`, so copy would silently extract nothing.
2. Its `render_line` never looks at `self.text_selection` at all — unlike the sibling `Log` widget, which does — so a drag over it shows **no visual highlight**, even when Screen-level selection tracking is otherwise working.
3. **The real reason a real mouse drag never worked at all**, not just gap 1/2: the compositor resolves a click's `(x, y)` back to a *content* position by reading `style.meta["offset"]` off each rendered segment (`_compositor.get_widget_and_offset_at`) — `Log` embeds this via `Strip.apply_offsets(scroll_x, y)` in its own `render_line`; `RichLog` never calls it. Without that metadata, `get_widget_and_offset_at` returns a widget but no content offset, so every real drag over a plain `RichLog` degrades to `Selection(None, None)` — the "select everything" sentinel — regardless of where the drag actually happened. This is the actual mechanism behind "a drag only ever highlights the phase sidebar": `PhasePanel` is a `Static`, which ratatui's generic `Widget.render_line`/`_styles_cache` path already knows how to offset; `RichLog`'s custom rendering path never was, no matter what `get_selection()`/`render_line` selection-painting does on top.

`OutputPane` (subclasses `RichLog`, used for every output/log pane) fixes all three: tracks a plain-text mirror of every `write()` call for `get_selection()`; overrides `render_line` to slice the rendered `Strip` at the selection's column span and `apply_style("screen--selection")` to the selected portion (mirroring `Log`'s highlight approach, adapted to `Strip`-based rather than `Text`-based line caching); and — the fix that actually matters for real mouse input — calls `strip.apply_offsets(scroll_x, absolute_y)` on every rendered line so clicks resolve to a precise content position instead of the select-all fallback. `absolute_y` (`scroll_y + y`) matters here: both the offset metadata and `selection.get_span()` must agree on content-absolute (scroll-adjusted) row numbering, or highlighting and click-resolution would disagree once the log has scrolled.

Forces `wrap=False` — `Selection` coordinates are row-indexed by *rendered* lines, and the plain-text mirror only tracks logical `write()` calls, not sub-line wrap points, so wrapping would misalign both extraction and highlight rendering; long lines scroll horizontally instead (mousewheel/keyboard scroll both unaffected — that's a plain `ScrollView` property, independent of `wrap`).

### Sticky scroll

New output pulls the view along while pinned to the bottom, but does not yank it back once the user scrolls up to read — and resumes following once they scroll back to the bottom. Driven by a sticky `_follow` flag on `OutputPane`, whose `write` passes `scroll_end=self._follow and self.text_selection is None` (RichLog's `scroll_end=None` sentinel is repurposed to mean "follow only while pinned to the bottom"). `_follow` is updated **only** in `watch_scroll_target_y` — which fires whenever the scroll *target* settles, from either our own follow-scroll (target → bottom) or a user scroll (wheel/scrollbar/keys) — as `round(new) >= max_scroll_y`.

**Active selection suspends follow.** The `and self.text_selection is None` guard is essential on a live streaming run: while the user is drag-selecting in the output pane, auto-follow would scroll the highlighted text out of the viewport (and stretch the selection as new lines slide under the cursor). The visible result is that a drag on the output pane "does nothing" — no highlight appears and the copied text is garbage — while the static phases pane highlights fine. This was the real cause behind repeated "I can only highlight the phases pane, not the output pane" reports: selection/`render_line`/`apply_offsets` were all correct (a static PTY drag resolved and painted perfectly), but streaming + follow scrolled the selection away. Freezing the view whenever a selection exists keeps the highlight put; follow resumes once the selection is cleared and the user scrolls back to the bottom. Only reproducible by streaming writes *during* a PTY drag — a static drag looks fine.

Two rejected approaches, and why:

- **ratatui's own `anchor()`** gets released by the resize/layout that fires right after mount in a real terminal (but not in the `run_test` harness), leaving `_anchor_released` stuck `True` — so the view never followed at all. This is why it "worked in tests" but not in a real terminal; it was only caught by driving the app in a real PTY.
- **Recomputing at-bottom from `scroll_offset.y` per write** fails on fast bursts: while following, RichLog scrolls *after* a refresh, so `scroll_offset.y` lags `max_scroll_y` by a line during a burst of writes between refreshes — a per-write position check reads "not at bottom" mid-stream and wrongly stops following. The sticky flag is only re-evaluated on an actual scroll settle, so the lag can't corrupt it.

Both the follow/stop/resume cycle and the fast-burst case are covered by tests, and the whole thing was verified end-to-end in a real PTY with actual SGR mouse-wheel escape sequences (wheel-up mid-stream stops following; wheel-down to the bottom resumes it).

### Output-pane selection highlight

A drag on the output pane produced no visible highlight (while the phases pane, a plain `Static`, highlighted fine) for a reason that hid behind two earlier partial fixes. The real cause: **the selection style was applied *under* the pane's own background.** `Strip.apply_style(style)` computes `style + segment.style` per segment (Rich's `Segment.apply_style(style=…)`), so a segment that already carries an explicit background — the output pane sets one (`#1c1c1c`) — keeps it, and the selection background (`#094472`) is discarded. The highlight rendered invisibly. Fix: apply the selection style as a **`post_style`** (`segment.style + post_style`) so it overrides the content background:

```python
middle = Strip(
    Segment.apply_style(list(selected), post_style=selection_style),
    selected.cell_length,
)
```

This is orthogonal to the two things that also made selection *feel* broken, both real and both fixed earlier: (a) `render_line`/`apply_offsets` were needed for the compositor to resolve a click to a content offset at all (without it, a drag degraded to the `Selection(None, None)` "select everything" sentinel); (b) sticky-follow scrolled the selection out of view on a streaming run (see Sticky scroll). All three had to be right before the output pane behaved.

**Why it kept slipping through tests:** the pilot-based unit tests called `render_line` directly and asserted "styles differ", which stayed true even with the bug (the fg still changed). The bug only manifests when the widget has its *own* background, and only shows up as *rendered* colour — neither of which a render_line-styles assertion on the plain harness log exercises. It is now caught by `test_selection_highlight_overrides_pane_background` (a log with an explicit background, asserting the selection bgcolor wins) and by the PTY tests below (asserting the highlight colour actually reaches the screen).

### Real-terminal test harness (flows-tui tests/pty.rs)

ratatui's `TestBackend` is an in-process fake — no PTY, so it misses real-terminal behaviour (the resize/layout right after mount, real SGR mouse decoding, actual cell rendering). The immediate-mode design avoids the retained-render bugs entirely, but a real-terminal harness (`tests/pty.rs`) forks the TUI binary under a real PTY (`portable-pty`) and feeds its output into `vt100`, an in-process VT100 emulator, so a test can send real keystrokes / SGR mouse events, let the app settle, and assert on the **rendered screen** — visible characters *and* per-cell background colours (via the `vt100` screen). "Is the selection highlight actually visible" is a question about rendered background colour, which `TestBackend` cannot answer and `vt100` can. The test binary is `src/bin/tui_demo.rs`; `portable-pty` and `vt100` are dev dependencies.

### Clipboard reliability

The underlying selection and its highlight now render correctly (see above), but two more things made "select + copy" appear broken:

1. **OSC 52 is unreliable.** an OSC 52 escape sequence alone is unreliable — fire-and-forget, silently dropped by macOS Terminal.app (noted in ratatui's own source), tmux without `set-clipboard on`, and some SSH setups. The clipboard path (via `arboard`) also shells out to the local clipboard tool — `pbcopy` (macOS), `wl-copy` (Wayland), `xclip`/`xsel` (X11), `clip` (Windows) —, landing text in the system clipboard on a local session regardless of OSC 52 support, plus a `notify()` toast so copy isn't silent.

2. **The copy key wasn't discoverable.** In a mouse-capturing TUI the terminal's own select+copy is disabled, so copy must go through the app via Ctrl+C — but muscle memory is to select and use the *terminal's* copy shortcut, which grabs nothing from a ratatui selection. `App` binds `ctrl+c → copy_selection` **visibly** (shown in the footer as "Copy sel"); with no selection it shows a hint ("Drag to select, then Ctrl+C … or 'y' to copy all") instead of failing silently. `y → copy_all` copies the entire visible output buffer (from the `OutputPane` plain-text mirror) — a mouse-free, one-key path that works even if the user never gets mouse selection to behave. Ctrl+C is deliberately not bound to quit (that would shadow copy, and a reflexive Ctrl+C quitting mid-run is rarely wanted); leaving is `q`/`h`.

The plain-text mirror that both copy-all and `get_selection()` read from is populated in `CopyableRichLog.write` only when `_size_known` is True: RichLog *defers* writes issued before its first resize and replays them later by calling `write()` again, so mirroring on both the original and the replay would double-count every early line (corrupting copy output and selection extraction). Gating on `_size_known` mirrors each line exactly once.

---

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
flows templates list                     # list all templates (builtin + user)
flows templates show <name>              # print template YAML
flows templates copy <name>              # copy to ~/.flows/templates/ for editing
flows templates copy <name> --to <path>
flows templates edit <name>              # open in $EDITOR (copies builtin first)
flows templates sync                     # refresh builtin templates/prompts into ~/.flows/; skips locally-modified files
flows templates sync --force             # also overwrite locally-modified files

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

- **Resource limits** on the sandbox (memory/pids/CPU via rlimits or cgroups).
- **Optional network isolation** for the shell sandbox (`--unshare-net`, opt-in for dep fetching).
- **Pipeline composition**: `include` another pipeline as a phase.
- **Conditional phases**: skip based on prior artifact content.
- **Brave / Serper** web-search backends (keyed).
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
