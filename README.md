# flows

**A local-first, terminal workflow runner for LLM coding agents.**

`flows` runs a multi-phase agent pipeline against your codebase — plan → implement → lint → test → review → commit — with every shell command confined to a sandbox, every file write scoped to the repo, and every run resumable from a TUI. It's built to run **fully offline against a local llama.cpp server** (no API key, no cloud, no cost), and also works with Anthropic or OpenAI models if you'd rather.

---

## What problem does it solve?

Ad-hoc "chat with an AI and paste code" loops are hard to trust, hard to repeat, and hard to supervise. `flows` makes agent work **structured, safe, and observable**:

- **Structured** — a run is a pipeline of named *phases* (agent / script / human), with conditional loop-backs, retries, and human gates. The same `feature` or `software` flow runs the same way every time; the lint/test commands adapt to your stack (cargo, pytest, npm, go).
- **Safe** — the agent's shell commands run in a filesystem sandbox (bubblewrap on Linux, Seatbelt on macOS): read anything, **write only inside the repo**, host secrets scrubbed from the environment, fail-closed if no sandbox is available. File tools are path-scoped (no `../` escape, no symlink-follow); web fetches are SSRF-guarded.
- **Observable** — live token streaming, per-phase token/cost accounting, a phase graph that shows loops, and full run state on disk. Start a run, walk away, and **reattach** to it later — running or finished — from the launcher.
- **Local-first** — point it at a llama.cpp (or any OpenAI-compatible) endpoint and run with zero cloud dependency and zero cost.

---

## Getting started

Follow these top to bottom — you'll have a run going in a couple of minutes.

### 1. Install

You need **Rust** (`curl https://sh.rustup.rs -sSf | sh`) and, on Linux, **bubblewrap** for the shell sandbox (`sudo apt install bubblewrap`). macOS uses the built-in `sandbox-exec`.

```bash
git clone <REPO_URL> flows && cd flows
cargo install --path crates/flows-cli --locked
```

This builds and installs the `flows` binary to `~/.cargo/bin` (make sure that's on your `PATH`). Templates and prompts are baked into the binary — nothing else to copy.

> Prefer not to install? `cargo build --release` and use `./target/release/flows` in place of `flows` below.

### 2. Point it at your local model (llama.cpp)

`flows` runs fully offline against any OpenAI-compatible server — **no API key, no cloud, no cost.** Start a llama.cpp server exposing the OpenAI API:

```bash
# in your llama.cpp checkout — serve a model on :8080 with the OpenAI-compatible API
llama-server -m /path/to/qwen2.5-coder.gguf --host 0.0.0.0 --port 8080
```

Then point `flows` at it:

```bash
flows config set model qwen2.5-coder                 # any name — llama.cpp serves whatever's loaded
flows config set api-base http://localhost:8080/v1
```

That's all — `flows` picks the OpenAI-compatible provider automatically because the model name isn't `claude*`, and no key is required when an `api_base` is set.

<details><summary>Prefer a cloud model instead?</summary>

```bash
export ANTHROPIC_API_KEY=sk-...          # or OPENAI_API_KEY for gpt-* models
flows config set model claude-sonnet-4-6
flows config set api-base ""             # clear any local endpoint
```
</details>

### 3. Check your setup

```bash
flows doctor
```

It prints the resolved model + where it routes, and whether the sandbox works:

```
model:    qwen2.5-coder  →  openai-compatible (local/custom)  http://localhost:8080/v1  [no key]
sandbox:  OK (bwrap)
```

If it says the sandbox probe **failed** (common on Ubuntu 23.10+, which blocks unprivileged user namespaces), `doctor` prints the exact fix — a one-line `sysctl`. Without it, `flows` still runs; only the `bash`/`plugin` tools fail closed.

> Tip: pick a **capable coding model** — the agent loop uses tool-calling, so a small/instruct model may struggle with the structured JSON responses. A larger coder model (e.g. Qwen2.5-Coder 14B/32B) works best. If runs stall or produce malformed artifacts, that's usually the model, not `flows`.

### 4. Run your first flow

`cd` into a git project, then either preview + run:

```bash
cd ~/some-project
flows plan "add a --json flag to the export command"   # dry-run: plan + cost, no LLM
flows run  "add a --json flag to the export command"   # actually run it
```

…or just launch the **interactive launcher** (no arguments):

```bash
flows
```

The launcher shows recent runs and a **＋ New run** button. Type an idea, pick a template, and it starts the run in a live full-screen view.

### 5. Watch, answer, and reattach

While a run is live you can:
- **scroll** the output (↑↓ / PgUp·PgDn / Home·End), **select + copy** (drag, then `Ctrl-C`; `y` copies all),
- **toggle** thinking (`t`) and JSON (`j`),
- **answer** a clarifying question or **approve** a human gate by typing in the bottom bar (Enter = approve),
- **cancel** with `c`, **quit** the view with `q`.

Press `q` to step away — the run keeps going in the background. Back at the launcher, select it again (or `flows attach <id>`) to **reattach** to the same live view. `flows runs` lists them with status + cost; `flows usage <id>` shows the token/cost breakdown. In the launcher, `d` deletes a selected finished run (or `flows clean` prunes them all).

That's it. Everything a run produces lives under `~/.flows/runs/<id>/`.

---

## Install — extras

Installation itself is step 1 of **Getting started** above. A few extras:

**Shell completions**
```bash
flows completions bash > ~/.local/share/bash-completion/completions/flows   # or zsh/fish
```

**Update** — `git pull && cargo install --path crates/flows-cli --locked`.

**Uninstall** — `cargo uninstall flows-cli`.

---

## Configuration

Global config lives in `~/.flows/config.yml`; a per-project `.flows.yml` overrides it. Set values with `flows config set`, or edit the file. A **llama.cpp** setup:

```yaml
llm:
  model: qwen2.5-coder                 # any name; llama.cpp serves whatever's loaded
  api_base: http://localhost:8080/v1   # your llama.cpp (OpenAI-compatible) endpoint
  thinking: false
max_cost_usd: 0.0                      # 0 = no cap (local is free anyway)
notifications:
  enabled: true
  sound: false
```

For a **cloud** model instead, drop `api_base` and set `model: claude-sonnet-4-6` (with `ANTHROPIC_API_KEY`) or a `gpt-*` model (with `OPENAI_API_KEY`).

- **Provider is chosen by model name:** `claude*` → Anthropic; anything else → OpenAI Chat Completions (which is what llama.cpp speaks). An `api_base` routes to your local/self-hosted endpoint — **no key required**.
- **API keys** (cloud only) come from `llm.api_key`, else `$ANTHROPIC_API_KEY`, else `$OPENAI_API_KEY`.

Generate a project config tailored to your stack:
```bash
flows init            # writes .flows.yml (detects Python / Rust / Node / Go)
flows init --global   # scaffold ~/.flows/config.yml + sync templates + prompts for customizing
```

---

## Commands

| Command | Description |
|---|---|
| `flows` | Interactive launcher — recent runs + new run, live attach |
| `flows run "<idea>"` | Run a pipeline (foreground). `--tui` full-screen, `--background` detached |
| `flows attach <id>` | Reattach to a run: phases, live output/artifacts, cost, answer prompts |
| `flows plan "<idea>"` | Validate config; show the execution plan + cost estimate (no LLM calls) |
| `flows runs` | List recent runs with status + cost |
| `flows delete <id> [--force]` | Delete one run by id (`--force` stops + deletes a running one) |
| `flows clean [--all]` | Delete finished runs from `~/.flows/runs` (`--all` also removes running ones) |
| `flows usage <id>` | Per-phase token usage + cost for a run |
| `flows doctor` | Check config, model routing, sandbox, templates |
| `flows config show \| set <key> <value>` | View / edit config (`model`, `api-base`, `api-key`, `max-cost`) |
| `flows templates list \| sync` | List templates; write templates + prompts to `~/.flows` |
| `flows init [--global]` | Generate `.flows.yml`, or sync global templates |
| `flows completions <shell>` | Print a shell completion script |

---

## How it works

A **template** defines a pipeline of **phases**:

- **agent** — an LLM tool loop. Tools are opt-in per phase: `files` (read/write/list/make_dir, repo-scoped), `bash` (sandboxed, allowlisted), `http_get` (SSRF-guarded), `web_search` (DuckDuckGo), and `plugin` commands. The agent sees a gitignore-aware repo tree and can ask clarifying questions.
- **script** — a trusted shell command (e.g. `lint`, `test`). Runs directly (no LLM), captures output. Supports the `@auto:*` resolvers below.
- **human** — a review gate; approve or send feedback. Supports `auto_advance`.

Phases can **loop back** on a condition (bounded by `max_loop_iterations`), **retry** on failure, and run as a **DAG** via `depends_on`. Shipped templates: `feature`, `software`, `research`, `review`, `blank`. Copy any to `.flows.yml` (or `~/.flows/templates/`) and customize; edit the synced prompts in `~/.flows/prompts/` to change agent instructions.

Every run writes to `~/.flows/runs/<id>/`: `state.json`, `meta.json`, `artifacts/*.yml`, `llm.jsonl` (usage/cost), and `output.log` (the live token stream).

---

## Template & configuration reference

A template (`.flows.yml`, or a file in `~/.flows/templates/`) is YAML with an
optional `llm:` block and a list of `phases:`.

```yaml
template: feature          # optional label
llm:                       # optional per-template model override
  model: claude-sonnet-4-6
phases:
  - name: plan
    type: agent
    ...
```

### Common phase fields (all types)

| field | default | meaning |
|---|---|---|
| `name` | *(required)* | phase id (used by `loop_back_to`, `depends_on`, `retry:`) |
| `type` | `agent` | `agent` \| `script` \| `human` |
| `on_failure` | `stop` | `stop` · `continue` · `retry:<phase>` (jump back to `<phase>`) |
| `depends_on` | `[]` | phases that must finish first — presence of any turns the pipeline into a **DAG** (parallel) |
| `loop_back_to` | — | phase to jump back to when `loop_condition` is true |
| `loop_condition` | — | MiniJinja expression over `content` (the phase's artifact); loop while true |
| `max_loop_iterations` | `3` | cap on `loop_back_to` iterations |

### `agent` phase

| field | default | meaning |
|---|---|---|
| `tools` | `files` | tool set — a preset name or an inline map (below) |
| `model` | *(from llm)* | override the model for this phase |
| `thinking` | *(from llm)* | enable extended thinking/reasoning |
| `prompt` | *(builtin)* | prompt override: a repo-relative `.md` path or a builtin key |
| `max_clarification_rounds` | `3` | rounds allowed for asking questions |
| `max_tool_rounds` | `20` | rounds allowed for tool use |

**Tool presets:** `files` · `files_and_test` · `files_and_deps` · `research` (http+search) · `research_ro` (read-only research) · `research_and_code` · `all`. Or configure inline:

```yaml
tools:
  files: true              # read_file / write_file / list_files / make_dir (repo-scoped)
  # read_only: true          # withhold write_file / make_dir (plan/review phases)
  bash:                    # sandboxed shell (bwrap/seatbelt)
    allowed: ["make*", "cargo*"]   # fnmatch allowlist
    # unrestricted: true            # allow any command (still sandboxed)
  http_get: true           # GET public URLs (SSRF-guarded)
  web_search: true         # DuckDuckGo
  plugins: ["my_tool"]     # custom plugin tools from ~/.flows/plugins
```

### `script` phase

| field | meaning |
|---|---|
| `command` | shell command; `{{ idea }}` is substituted (MiniJinja). Runs directly, not via the LLM. |

**`@auto` resolvers** — resolved in Rust at run time (no tokens): prefer a `make`
target, else the language's tool; **the run stops if no tool is detectable**.

```yaml
- name: test
  type: script
  command: "@auto:test"        # make test → cargo test / pytest / npm run test / go test / …
  on_failure: retry:implement  # a real test failure loops back; an unresolvable tool stops the run
- name: lint
  type: script
  command: "@auto:lint"        # make lint → cargo clippy / ruff / npm run lint / go vet / …
```

### `human` phase

| field | default | meaning |
|---|---|---|
| `instructions` | *(generic)* | text shown at the gate |
| `auto_advance` | `false` | skip the interactive gate (proceed automatically) |

At a human gate you press **Enter** to approve or type feedback; feedback is
available to a `loop_condition` (e.g. `content.get('answer','').strip() not in ('','yes','approved')`).

### Loop conditions & the `{{ idea }}` var

- `loop_condition` and `script` commands are **MiniJinja** (Jinja-compatible). Python idioms resolve natively: `.get('k')`, `.get('k', default)`, `.strip()`, `.startswith(...)`, `x in (...)`, and the `tojson` filter.
- `content` in a `loop_condition` is the phase's artifact content (a map).
- `{{ idea }}` (the run's idea text) is available in `script` commands.

---

## Agent prompt templates

An agent's system prompt is a MiniJinja template. Lookup order (first match wins):
`<repo>/.flows/prompts/<template>/<phase>.md` → `~/.flows/prompts/<template>/<phase>.md`
→ builtin. `flows templates sync` writes the builtins to `~/.flows/prompts/` so you can edit them.

**Variables available in a prompt:**

| variable | value |
|---|---|
| `idea` | the run's idea / task text |
| `repo_tree` | gitignore-aware file tree of the repo (vendor dirs skipped) |
| `phase_name` | the current phase's name |
| `prior_artifacts` | map of `{phase_name: artifact}` for all completed phases |
| `<phase_name>` | each prior artifact is also exposed by its own name (e.g. `{{ plan.content.summary }}`) |

Same MiniJinja idioms as above (`.get`, `.strip`, `tojson`, …).

**`AGENTS.md`** — the cross-tool convention for repo instructions. If present,
`~/.flows/AGENTS.md` (global) and `<repo>/AGENTS.md` (project, wins) are appended
to **every** agent's system prompt. Use it for house rules like *"run all tooling
through `make`."*

---

## Security model

- **Shell sandbox** (`bash`, `plugin`): whole filesystem read-only, repo + a private `/tmp` writable, host environment cleared (no API keys leak into agent commands), namespaces isolated. **Fail-closed** — if no sandbox is available, these tools error rather than run unconfined. Backends: bubblewrap (Linux), Seatbelt (macOS).
- **File tools**: resolved and confined under the repo root; `../` traversal, absolute paths, and symlink escapes are rejected, and the target is opened `O_NOFOLLOW` to defeat symlink races.
- **Network**: `http_get`/`web_search` block private, loopback, link-local, CGNAT, and IPv4-mapped-IPv6 addresses (cloud metadata endpoints included), re-validating on every redirect hop.
- **Shell network**: the `bash` tool rejects ad-hoc network — raw clients (`curl`/`wget`/`nc`/`ssh`/…) and inline interpreter eval (`python -c`, `node -e`, piping/heredoc into an interpreter) — so an agent command can't bypass the SSRF-guarded tools. Dependency managers (`pip`/`cargo`/`npm`/`go`) still fetch deps; a trusted `script` phase (not gated) can run a raw network command if you truly need one.
- **Cost**: `max_cost_usd` caps spend; the run stops when exceeded.

> Note: the sandbox itself keeps network up (so builds can fetch dependencies), so the shell-network *denylist* above is a defense-in-depth speed-bump, not an airtight boundary — a `make` recipe or a malicious dependency can still reach the network transitively. Host-level egress filtering (allowlist package registries only) is a planned follow-up; disable the `bash` tool for a phase if you need hard isolation today.

---

## Development

```
crates/
  flows-core     config, state store, templates, pricing, usage, detection
  flows-tools    files, sandbox (bwrap/seatbelt), bash, plugin, http, search
  flows-llm      provider trait + Anthropic / OpenAI translation & streaming
  flows-runner   phases, agent tool loop, prompts, pipeline (linear + DAG)
  flows-tui      ratatui launcher, run view, attach view
  flows-cli      the `flows` binary
```

```bash
cargo test               # full suite
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

CI runs fmt + clippy (deny warnings) + tests on every push.
