# flows

**A local-first, terminal workflow runner for LLM coding agents.**

`flows` runs a multi-phase agent pipeline over your codebase — plan → implement →
lint → test → review → commit — in a sandbox, from a resumable full-screen TUI.
It's built to run **fully offline against a local llama.cpp server** (no API key,
no cloud, no cost), and also works with Anthropic or OpenAI models.

---

## Why

Ad-hoc "chat with an AI and paste code" loops are hard to trust, repeat, and
supervise. `flows` makes agent work **structured, safe, and observable**:

- **Structured** — a run is a pipeline of named *phases* (agent / script / human)
  with loop-backs, retries, and human gates. The same `feature` or `software` flow
  runs the same way every time; lint/test commands adapt to your stack (cargo,
  pytest, npm, go).
- **Safe** — the agent's shell runs in a filesystem sandbox (bubblewrap / Seatbelt):
  **write only inside the repo**, credential stores (`~/.ssh`, `~/.aws`, API keys, …)
  hidden, no host IPC, fail-closed if no sandbox. File tools can't escape the repo; web access is SSRF-guarded.
- **Observable** — live token streaming, per-phase token/cost accounting, a phase
  graph that shows loops, full run state on disk. Start a run, walk away, and
  **reattach** later — running or finished.
- **Local-first** — point it at a llama.cpp (or any OpenAI-compatible) endpoint and
  run with zero cloud dependency and zero cost.

---

## Quickstart

### 1. Install & set up

Needs **Rust** and, on Linux, **bubblewrap** for the sandbox. macOS uses the
built-in sandbox.

```bash
# prerequisites
curl https://sh.rustup.rs -sSf | sh        # Rust
sudo apt install bubblewrap                # Linux only

# build + install the `flows` binary to ~/.cargo/bin (must be on PATH)
git clone <REPO_URL> flows && cd flows
cargo install --path crates/flows-cli --locked

# scaffold your local config + editable templates/prompts in ~/.flows/
flows init --global
```

`flows init --global` writes a commented `~/.flows/config.yml` and copies the
templates + agent prompts into `~/.flows/` so you can tweak them. (The binary also
has them baked in, so flows works before you customize anything.)

### 2. Point it at a model

Edit `~/.flows/config.yml`, or use `flows config set`:

**Local (llama.cpp / vLLM / any OpenAI-compatible server) — no key, no cost:**

```bash
# serve a model on :8080 (in your llama.cpp checkout)
llama-server -m /path/to/qwen2.5-coder.gguf --host 0.0.0.0 --port 8080

flows config set model qwen2.5-coder                 # any name; serves what's loaded
flows config set api-base http://localhost:8080/v1
```

**Or cloud:**

```bash
export ANTHROPIC_API_KEY=sk-...          # or OPENAI_API_KEY for gpt-* models
flows config set model claude-sonnet-4-6
flows config set api-base ""             # clear any local endpoint
```

> Pick a **capable coding model** — the agent loop uses tool-calling, so tiny
> instruct models struggle with structured responses. Qwen2.5-Coder 14B/32B or a
> cloud model works well.

### 3. Check your setup

```bash
flows doctor
```

Prints the resolved model + routing and whether the sandbox works. If the sandbox
probe **fails** (common on Ubuntu 23.10+, which blocks unprivileged user
namespaces), `doctor` prints the one-line `sysctl` fix.

### 4. Run your first flow

```bash
cd ~/some-project                                    # a git repo
flows plan "add a --json flag to the export command" # dry-run: plan + cost, no LLM
flows run  "add a --json flag to the export command" --tui
```

…or just launch the interactive picker with no arguments:

```bash
flows
```

### 5. Watch, answer, detach

While a run is live in the TUI:

- **scroll** (↑↓ / PgUp·PgDn / Home·End), **select + copy** (drag then `Ctrl-C`; `y`
  copies all),
- **toggle** thinking (`t`), JSON (`j`), system prompt (`s`),
- **answer** a question or **approve** a gate in the bottom bar (Enter = approve),
- **`q`** detaches — *the run keeps going in the background*.

Reattach with `flows attach <id>` (or pick it from `flows`). Stop a run with
`flows stop <id>` (or `c` then `q` in the TUI). Everything a run produces lives under
`~/.flows/runs/<id>/`.

---

## Everyday commands

| Command | Description |
|---|---|
| `flows` | Interactive launcher — recent runs + new run, live attach |
| `flows run "<idea>" [--tui] [--background] [-C DIR]` | Run a pipeline. `--tui` = live view (detaches on `q`); `-C/--directory DIR` targets another project (like `git -C`) |
| `flows attach <id>` | Reattach to a run: phases, live output, cost, answer prompts |
| `flows stop <id>` | Cancel a run and kill its worker (keeps the run + output) |
| `flows plan "<idea>"` | Validate config; show the execution plan + cost estimate (no LLM) |
| `flows runs` | List recent runs with status + cost |
| `flows delete <id> [--force]` | Delete one run (`--force` stops + deletes a running one) |
| `flows clean [--all]` | Delete finished runs (`--all` also removes running ones) |
| `flows usage <id>` | Per-phase token usage + cost for a run |
| `flows doctor` | Check config, model routing (probes local servers), sandbox, templates (flags stale `~/.flows` copies) |
| `flows config show \| set <key> <value>` | View / edit config |
| `flows init [--global]` | Generate `.flows.yml`, or scaffold global config + templates |
| `flows templates list \| sync \| new <name>` | List templates; copy builtins to `~/.flows` to edit; scaffold a commented new one |
| `flows templates install <git>[//path][@ref]` | Install a shared template, pinned and reviewed; scripts sandboxed until trusted ([docs](./docs/templates.md#installing-templates-from-git)) |
| `flows searxng up \| down \| status` | Manage an optional self-hosted SearXNG for web search |
| `flows eval …` | Benchmark models on task fixtures — see [docs/eval.md](./docs/eval.md) |
| `flows completions <shell>` | Print a shell completion script |

**Update:** `git pull && cargo install --path crates/flows-cli --locked`.
**Uninstall:** `cargo uninstall flows-cli`.

---

## Configure

Global config lives in `~/.flows/config.yml`; a per-project `.flows.yml` overrides
it. A minimal local setup:

```yaml
llm:
  model: qwen2.5-coder
  api_base: http://localhost:8080/v1
max_cost_usd: 0.0        # 0 = no cap (local is free)
```

The provider is chosen by model name (`claude*` → Anthropic; else OpenAI-compatible,
which llama.cpp speaks). See **[docs/configuration.md](./docs/configuration.md)** for
per-phase model profiles, no-tool models, the `web_search` / `searxng` / `lookup`
tools, and context-delivery features for small models.

---

## Documentation

- **[Configuration](./docs/configuration.md)** — models, per-phase profiles, search
  & lookup tools, SearXNG, context delivery.
- **[Templates, phases & prompts](./docs/templates.md)** — the pipeline format,
  phase fields, tool presets, prompt variables.
- **[Benchmarking (`flows eval`)](./docs/eval.md)** — measure how well a model
  drives the agent loop.
- **[Architecture & security](./docs/architecture.md)** — how a run works, crate
  layout, the sandbox + network model.

---

## Development

Use the `make` targets (the agent flows + `AGENTS.md` expect `make`, not bare cargo):

```bash
make check     # fmt-check + lint + test (the full gate)
make test      # cargo test --workspace
make lint      # clippy, warnings denied
make fmt       # format
make build     # build the workspace
make install   # install the flows CLI to ~/.cargo/bin
```

Crate layout and internals: **[docs/architecture.md](./docs/architecture.md)**.
