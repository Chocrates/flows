# Porting `flows` to Rust — implementation plan

Status: proposal. Target: a single statically-linked binary replacing the Python
package (no venv/pipx, faster startup, stronger types), at feature parity with
today's `flows`.

Current Python surface: ~6.2k LOC source, ~8.4k LOC tests, 9 runtime deps.

---

## 1. Why / why not

**Wins:** one static binary (drop pipx + Python env entirely), fast cold start,
real threads instead of asyncio, exhaustive `match` over the tri-state config and
phase-type unions, `serde` for all the on-disk JSON/YAML.

**Costs / risks (the honest part):**
- **No litellm.** The unified multi-provider LLM client (streaming, tool calls,
  cost, extended thinking, the Anthropic empty-content quirk) is our single
  biggest dependency and has no drop-in Rust equivalent. This is long pole #1.
- **No Textual.** `ratatui` is immediate-mode; Textual is retained/reactive. The
  custom output pane (mouse text-selection, sticky-follow scroll, highlight
  compositing, clipboard) must be rebuilt from primitives. Long pole #2 — and
  it's where most of this session's hard-won bug fixes live.
- Token counting for `estimate` has no local Anthropic tokenizer.

Everything else (config, state, tools, sandbox, runner, templates, pricing)
ports cleanly.

---

## 2. Dependency mapping

| Python | Rust | Notes |
|---|---|---|
| `litellm` | `genai` crate, or hand-rolled `reqwest` + `eventsource-stream` | Multi-provider + streaming + tools. `genai` is closest to litellm; fall back to per-provider reqwest if it lacks thinking/tool-call fidelity. |
| `typer` | `clap` (derive) | 1:1. |
| `rich` (plain output) | `owo-colors` / `console` | Non-TUI colored output. |
| `textual` | `ratatui` + `crossterm` | See §5. |
| `pyyaml` | `serde_yaml_ng` (serde_yaml is unmaintained) | With `serde`. |
| `jinja2` | `minijinja` | Near-Jinja2, has a sandboxed environment (for plugin templates). |
| `pathspec` | `ignore` crate (ripgrep) | .gitignore matching for repo tree/context. |
| `httpx` | `reqwest` | `http_get` tool + provider calls. |
| `duckduckgo-search` | hand-rolled `reqwest` (DDG lite/html endpoint) | Small; no maintained crate. |
| asyncio | `tokio` | Runtime. |
| `subprocess` (bwrap) | `std::process::Command` | Same argv — trivial. |
| clipboard (OSC52) | `arboard` + OSC52 fallback | `arboard` talks to the OS clipboard directly — strictly better than our shell-out to pbcopy/wl-copy/xclip. |
| desktop notify | `notify-rust` | Replaces `notify.py`. |
| `pytest` / `pyte` | `cargo test` + `rstest`; `portable-pty` + `vt100` | PTY harness ports directly (vt100 == pyte). |
| tokenizer (estimate) | `tiktoken-rs` (OpenAI); approximate Anthropic | See §6. |

---

## 3. Workspace layout

Cargo workspace, crates mirroring the Python packages so the port is
module-for-module reviewable:

```
flows/
  crates/
    flows-core/     # models, config (3-tier merge), state, pricing, usage, templates, detect
    flows-llm/      # provider client: completion, streaming, tool calls, thinking, cost
    flows-tools/    # files, sandbox (bwrap), bash, http, search, plugin
    flows-runner/   # pipeline exec (linear + DAG), phases (agent/script/human), signals
    flows-tui/      # ratatui UI
    flows-cli/      # clap binary; init/doctor/run/templates/estimate/dry-run; background worker
  templates/        # default_templates/*.yml + prompts/**  (embedded via include_dir)
```

`flows-core` has no I/O beyond fs; everything depends inward on it. `flows-tui`
and `flows-llm` are the only crates with real complexity.

---

## 4. What ports cleanly (do these first — they de-risk the rest)

- **models.py → serde structs.** `PhaseConfig`, `ToolsConfig`, `LLMConfig`, etc.
  Tri-state `bool | None` → `Option<bool>`. Phase `type` → an enum. `max_tool_rounds:
  Option<u32>`. This is the schema the whole system hangs off.
- **config.py → 3-tier merge.** global → user-project-override → project. The
  `None = inherit` merge logic becomes explicit `Option` combinators. Env
  expansion (`${VAR}`) via a small pass. Well-covered by tests; port those first.
- **state.py + signals.** run dirs, `state.json`, `meta.json`, `llm.jsonl`,
  `signals/`, `artifacts/`. `serde_json` + `std::fs`. Cooperative cancel/answer/
  advance = signal files. Immediate cancel = `nix::sys::signal::kill(pid, SIGTERM)`.
- **sandbox.py (bwrap).** Identical bwrap argv via `Command`; probe + cache;
  fail-closed. The one security-critical module, and it's a near-verbatim port.
- **tools/files.py.** `_safe_path` = `repo_root.join(p).canonicalize()` then
  `starts_with(repo_root)`. `ignore` crate for the repo tree/context loader.
- **tools/http.py, search.py, plugin.py.** reqwest + the SSRF/private-IP guard
  (also fix the known redirect-revalidation gap while porting). minijinja
  sandboxed env for plugin command templates, then through `run_confined`.
- **pricing.py, usage.py, dryrun.py, estimate.py, detect.py, notify.py.** Pure
  logic / table lookups; direct translations.
- **templates.py.** Embed `default_templates/` + `prompts/` with `include_dir!`;
  `init`/`templates sync` write them to `.flows/` / `~/.flows/`.

---

## 5. Long pole #1 — the TUI (`flows-tui`)

Textual is reactive/retained; ratatui redraws every frame from state you own.
Rebuild, don't translate. Port target = today's behavior, which we already
reverse-engineered and documented in DESIGN.md. Treat those as the spec:

- **Layout:** phases pane (left) + output pane (right), input bar, header/footer.
  `crossterm` for input + raw mode + mouse capture (SGR already, matches our PTY
  tests).
- **Output pane = the hard widget.** ratatui has no text selection. We own:
  - a scrollback buffer of styled lines + a plain-text mirror (for copy/extract);
  - **sticky-follow scroll**: follow the bottom unless the user scrolled up;
    suspend follow while a selection exists (our streaming-selection fix);
  - **mouse drag selection**: map SGR mouse (col,row) → buffer offset accounting
    for scroll; this is trivial in immediate mode (we control the mapping) — no
    `apply_offsets`/compositor fight like Textual;
  - **highlight rendering**: paint the selected span with a distinct bg — and
    make sure the selection style *overrides* the pane background (the
    `post_style` bug — in ratatui, just set the cell style directly);
  - **copy**: `arboard` to the OS clipboard + OSC52 fallback; `y` copy-all,
    ctrl+c copy-selection.
  - **categorized output + toggles**: thinking / JSON / normal, `t`/`j` toggles,
    re-render from the retained entries list.
- **Phases pane:** status glyphs, looping indicator, loop-iteration counts.
- **Attach/background:** poll `state.json` for backgrounded runs; questions and
  human-gate input; force-cancel.

Note: most Textual-specific bugs we fixed (retained-render caching, compositor
offset resolution, anchor-released-on-resize) **do not exist** in immediate-mode
ratatui — we redraw from truth each frame. New risks instead: redraw
performance under streaming, and getting scroll math right. The PTY+vt100
harness (below) is how we verify pixel-for-cell parity.

---

## 6. Long pole #2 — the LLM client (`flows-llm`)

Replaces litellm. Responsibilities: multi-provider chat completion, SSE
streaming, tool-call assembly, extended thinking, usage/cost, and the quirks we
depend on.

- **Providers:** Anthropic + OpenAI-compatible to start (that covers the default
  models). Evaluate the `genai` crate first; if thinking/tool fidelity is
  insufficient, hand-roll with `reqwest` + `eventsource-stream` per provider.
- **Must replicate, verbatim behavior:**
  - streaming token accumulation + tool-call argument assembly across chunks;
  - tool-call turn assistant message uses `content: null` not `""` (the Anthropic
    "[System: Empty message content sanitised…]" placeholder bug);
  - suppress questions-JSON from the visible token stream;
  - `reasoning_content`/thinking blocks forwarded to `on_thinking`;
  - `thinking={type:enabled, budget_tokens}` + `max_tokens > budget`.
- **Cost:** per-response cost from `pricing` table; feeds the mid-phase cost cap.
- **Estimate/token counting:** `tiktoken-rs` for OpenAI-family; Anthropic has no
  local tokenizer — either call the count-tokens endpoint or keep the current
  heuristic (chars/4 + per-phase output guess). Document the approximation.

The agent loop itself (`phases/agent.py`) — round budget (`max_clarification +
max_tool_rounds`), degenerate-loop guard, mid-phase cost abort, tool dispatch,
questions/artifact parsing — is straightforward control flow over this client.

---

## 7. Runner (`flows-runner`)

Direct port of `runner.py`: linear + DAG execution, `loop_back_to`/
`loop_condition` (minijinja expr eval), `max_loop_iterations`, `on_failure`
(stop/continue/retry), `ScriptFailed`/`AgentFailed` → Rust `enum RunError` with
carried artifact, per-phase-boundary + mid-phase cost checks, cancel polling.
`tokio::select!`/`JoinSet` for DAG parallelism. Callbacks (`RunnerCallbacks`) →
a trait with foreground/background/TUI impls.

Background worker (`_worker.py`): spawn a detached child (`Command`, new session)
running the pipeline headless, writing state; TUI attaches by polling.

---

## 8. Milestones (each independently shippable)

- **M0 — Core, no LLM.** Workspace + CI. models/config/state/templates/pricing/
  usage/detect. `flows init`, `doctor`, `templates sync`, `dry-run`, `estimate`.
  Port the config/state/template test suites. *Exit:* config + scaffolding at parity.
- **M1 — Headless agent.** LLM client (non-streaming) + tools + linear runner +
  agent/script/human phases + plain foreground output. *Exit:* `feature` flow runs
  end-to-end in a terminal, no TUI.
- **M2 — Streaming & guards.** SSE streaming, thinking, tool-call summaries,
  mid-phase cost cap, degenerate-loop guard, `max_tool_rounds`.
- **M3 — Orchestration.** DAG, loops, on_failure/retry, cancel/answer/advance
  signals, background worker + attach.
- **M4 — TUI.** ratatui port per §5, incl. selection/sticky-scroll/clipboard/
  toggles/cancel. Verified with the PTY harness.
- **M5 — Parity & polish.** Remaining tests, PTY harness, doctor tool detection,
  notify, docs. Fix the http redirect-SSRF gap in passing.

Headless (M0–M3) is usable well before the TUI lands.

## 9. Testing

- Unit/integration → `cargo test` + `rstest` (parametrized cases like the
  blocklist/allowlist tables). Aim to carry over the existing coverage intent.
- **PTY harness → `portable-pty` + `vt100`.** `vt100::Parser` is pyte's twin:
  spawn the binary under a real PTY, drive SGR mouse/keys, assert on rendered
  cells + colors. Port `tests/test_pty_output_selection.py` — it's the parity
  gate for the output pane (selection highlight, sticky-follow-during-stream).
- Real bwrap containment test — same skip-if-unavailable gate.
- Snapshot tests (`insta`) for rendered TUI frames.

## 10. Effort & sequencing

Rough: ~6.2k Py LOC → ~8–12k Rust LOC. Long poles dominate. Solo, roughly:
M0–M3 (headless core) ≈ 3–5 weeks; M4 (TUI) ≈ 2–4 weeks; M5 ≈ 2 weeks →
**~8–12 weeks**, but the headless binary is dogfoodable from end of M1.

De-risk early: build a throwaway spike of (a) `genai`/reqwest streaming a tool
call from Anthropic incl. thinking, and (b) a ratatui output pane with mouse
drag-selection + clipboard, **before** committing to the full port. Those two
spikes retire ~80% of the uncertainty.

## 11. Open decisions

- `genai` crate vs hand-rolled provider clients (spike first).
- Single tokio-based binary for both foreground and background, or a separate
  worker binary (Python uses a `_worker` entrypoint).
- Keep the shell-out clipboard *and* add `arboard`, or `arboard`-only.
- Anthropic token counting: API call vs heuristic for `estimate`.
