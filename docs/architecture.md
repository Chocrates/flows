# Architecture & security

## How a run works

A **template** defines a pipeline of **phases** (see [templates](./templates.md)):

- **agent** — an LLM tool loop. Tools are opt-in per phase: `files`
  (read/write/list/make_dir, repo-scoped), `bash` (sandboxed, allowlisted),
  `http_get` (SSRF-guarded), `web_search` (DuckDuckGo/SearXNG), `lookup`
  (crates.io/docs.rs/…), and `plugin` commands.
- **script** — a trusted shell command (`lint`, `test`), run directly, output
  captured. Supports `@auto:*` resolvers.
- **human** — a review gate; approve or send feedback.

Phases can **loop back** on a condition (bounded by `max_loop_iterations`),
**retry** on failure, and run as a **DAG** via `depends_on`.

Every run writes to `~/.flows/runs/<id>/`:

- `state.json` — status, current phase, questions, completed phases,
- `meta.json` — idea, template, repo path, worker pid,
- `artifacts/*.yml` — each phase's structured output,
- `llm.jsonl` — per-round usage/cost,
- `output.log` — the categorized live token stream (what `attach` replays).

### Foreground vs background

`flows run --tui` and the launcher both run the pipeline as a **detached background
worker** and attach a live view to it. Quitting the view (`q`) only *detaches* — the
run keeps going; reattach with `flows attach <id>`, stop it with `flows stop <id>`.
The worker streams to `output.log`, so a reattach replays the whole session.

`flows run` without `--tui` runs in the foreground and prints as it goes.

## Crate layout

```
crates/
  flows-core     config, state store, templates, pricing, usage, detection
  flows-tools    files, sandbox (bwrap/seatbelt), bash, http, search, lookup, plugin
  flows-llm      provider trait + Anthropic / OpenAI translation & streaming
  flows-runner   phases, agent tool loop, prompts, context delivery, pipeline (linear + DAG)
  flows-tui      ratatui launcher, run view, attach view
  flows-cli      the `flows` binary
```

Use the `make` targets (the agent flows + `AGENTS.md` policy expect `make`):

```bash
make check     # fmt-check + lint + test — the full gate
make test      # cargo test --workspace
make lint      # clippy --workspace --all-targets -- -D warnings
make fmt       # cargo fmt --all
make build     # cargo build --workspace
make install   # install the flows CLI to ~/.cargo/bin
```

CI runs fmt + clippy (deny warnings) + tests on every push.

---

## Security model

- **Shell sandbox** (`bash`, `plugin`): whole filesystem read-only, repo + a
  private `/tmp` writable, host environment cleared (no API keys leak into agent
  commands), namespaces isolated. **Fail-closed** — if no sandbox is available,
  these tools error rather than run unconfined. Backends: bubblewrap (Linux),
  Seatbelt (macOS).
- **Toolchains** work inside the sandbox without leaking secrets: `HOME=/tmp` hides
  credential-bearing dotfiles (`~/.npmrc`, `~/.netrc`, cloud creds), so instead
  flows passes through only *locator* env vars (where a toolchain / version manager
  is installed — `RUSTUP_HOME`, `GOROOT`/`GOPATH`, `JAVA_HOME`, `NVM_DIR`,
  `PYENV_ROOT`, `SDKMAN_DIR`, …) and redirects each ecosystem's cache
  (cargo/go/npm/pip/gradle/maven) to a persistent writable dir
  (`~/.cache/flows/toolcache`) bound into the sandbox. Builds resolve their
  compiler and reuse downloaded dependencies across calls.
- **File tools**: resolved and confined under the repo root; `../` traversal,
  absolute paths, and symlink escapes are rejected, and the target is opened
  `O_NOFOLLOW` to defeat symlink races.
- **Network**: `http_get` / `web_search` / `lookup` block private, loopback,
  link-local, CGNAT, and IPv4-mapped-IPv6 addresses (cloud metadata endpoints
  included), re-validating on every redirect hop.
- **Shell network**: the `bash` tool rejects ad-hoc network — raw clients
  (`curl`/`wget`/`nc`/`ssh`/…) and inline interpreter eval (`python -c`, `node -e`,
  piping into an interpreter) — so an agent command can't bypass the SSRF-guarded
  tools. Dependency managers (`pip`/`cargo`/`npm`/`go`) still fetch deps; a trusted
  `script` phase can run a raw network command if you truly need one.
- **Cost**: `max_cost_usd` caps spend; the run stops when exceeded.

> The sandbox keeps network up (so builds can fetch dependencies), so the
> shell-network *denylist* is a defense-in-depth speed-bump, not an airtight
> boundary — a `make` recipe or a malicious dependency can still reach the network
> transitively. Disable the `bash` tool for a phase if you need hard isolation.
