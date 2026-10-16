# Templates, phases & prompts

A **template** defines a pipeline of **phases**. Shipped: `feature` (plan → build →
review), `software` (spec-driven, from scratch), `research`, `review`, `plan`
(planning/analysis only), `implement`, `blank`.

## Getting your own templates

Templates resolve in order: `<repo>/.flows.yml` → `~/.flows/templates/<name>.yml` →
builtin. Three ways to author one:

```bash
flows templates list          # what's available (project → global → builtin)
flows templates sync          # copy ALL builtins into ~/.flows/templates to edit
flows templates new <name>    # scaffold a fresh, heavily-commented template
```

- **`sync`** drops editable copies of every shipped template (and the agent
  prompts) into `~/.flows/`. Edit in place; your copy wins over the builtin.
  `--force` overwrites.
- **`new <name>`** writes `~/.flows/templates/<name>.yml` — one of each phase type
  with every common field commented, so you learn the format by editing. Run it
  with `flows run "…" --template <name>`.
- For a **single project**, put a `.flows.yml` at the repo root (`flows init`
  detects your stack and generates one).

A template is YAML with an optional `llm:` block and a list of `phases:`:

```yaml
template: feature          # optional label
llm:                       # optional per-template model override
  model: claude-sonnet-4-6
phases:
  - name: plan
    type: agent
    ...
```

## Phase types

- **agent** — an LLM tool loop. The agent sees a gitignore-aware repo tree and can
  ask clarifying questions.
- **script** — a trusted shell command (e.g. `lint`, `test`). Runs directly (no
  LLM), captures output. Supports the `@auto:*` resolvers below.
- **human** — a review gate; approve or send feedback.

Phases can **loop back** on a condition (bounded by `max_loop_iterations`),
**retry** on failure, run **conditionally** via `when`, and run as a **DAG** via
`depends_on`.

## Common phase fields (all types)

| field | default | meaning |
|---|---|---|
| `name` | *(required)* | phase id (used by `loop_back_to`, `depends_on`, `retry:`) |
| `type` | `agent` | `agent` \| `script` \| `human` |
| `on_failure` | `stop` | `stop` · `continue` · `retry:<phase>` |
| `depends_on` | `[]` | phases that must finish first — any presence makes the pipeline a **DAG** (parallel) |
| `loop_back_to` | — | phase to jump back to when `loop_condition` is true |
| `loop_condition` | — | MiniJinja expression over `content` (the phase's artifact); loop while true |
| `max_loop_iterations` | `3` | cap on `loop_back_to` iterations |
| `when` | — | MiniJinja expression over `artifacts.<phase>` and `idea`; the phase runs only when it is true (see below) |

## `agent` phase

| field | default | meaning |
|---|---|---|
| `tools` | `files` | tool set — a preset name or an inline map (below) |
| `model` | *(from llm)* | override the model for this phase |
| `thinking` | *(from llm)* | enable extended thinking/reasoning |
| `prompt` | *(builtin)* | prompt override: a repo-relative `.md` path or a builtin key |
| `max_clarification_rounds` | `3` | questions the phase may ask the human (counted per question, not per round) |
| `max_tool_rounds` | `40` | tool-use rounds allowed |

**Tool presets:** `files` · `files_and_test` · `files_and_deps` · `research`
(http + search + lookup) · `research_ro` (read-only research) · `research_and_code`
(implement: build + web) · `all`. Any preset that grants `web_search` also grants
`lookup`. Or configure inline:

```yaml
tools:
  files: true              # read_file / write_file / list_files / make_dir (repo-scoped)
  # read_only: true          # withhold write_file / make_dir (plan/review phases)
  bash:                    # sandboxed shell (bwrap/seatbelt)
    allowed: ["make*", "cargo*"]   # fnmatch allowlist
    # unrestricted: true            # allow any command (still sandboxed)
    # timeout_secs: 1800            # per-command timeout (default 900s; cold builds are slow)
    # memory_mb: 16384              # memory cap per command tree (default 8192; 0 = none)
    # max_procs: 2048               # process/thread cap per command (default 1024; 0 = none)
    # expose_credentials: true      # let commands read ~/.ssh, ~/.aws, … (hidden by default)
    # offline: true                 # no network for commands (loopback only); default off
  http_get: true           # GET public URLs (SSRF-guarded)
  web_search: true         # DuckDuckGo or SearXNG (see configuration.md)
  lookup: true             # crates.io / docs.rs / stackexchange / wikipedia / github
  plugins: ["my_tool"]     # custom plugin tools from ~/.flows/plugins
```

Resource limits use a per-command systemd user scope (cgroup) on Linux. Without
one they fall back to a per-process memory rlimit, and on macOS they aren't
enforced. Commands always still run; `flows doctor` shows which mode applies.
A command killed by the memory cap says so in its output.

Credential stores under your home directory are hidden from sandboxed commands
by default: an empty directory or file appears in place of `~/.ssh`, `~/.gnupg`,
`~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.kube`, `~/.docker`,
`~/.config/gh`, `~/.netrc`, `~/.git-credentials`, `~/.npmrc`, `~/.pypirc`,
cargo credentials, password stores and keyrings, browser profiles, and
AI-assistant configs (full list: `CREDENTIAL_PATHS` in
`crates/flows-tools/src/sandbox/mod.rs`). A phase whose build needs them, e.g.
git-over-ssh dependencies or a private registry, sets
`tools.bash.expose_credentials: true`; a plugin sets `expose_credentials: true` in
its YAML. flows' own `~/.flows/config.yml` and `~/.flows/projects/`, which hold
API keys, stay hidden either way. Script phases are not sandboxed and see
everything.

`offline: true` cuts the network for a phase's commands, which suits test, lint and
review phases that don't fetch anything. On Linux the command gets its own
loopback, so it can't reach services on the host's `127.0.0.1` either. On macOS
it is limited to `localhost`, which is still shared with the host. The model is
told the shell is offline.

## `script` phase

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

## `human` phase

| field | default | meaning |
|---|---|---|
| `instructions` | *(generic)* | text shown at the gate |
| `auto_advance` | `false` | skip the interactive gate (proceed automatically) |

At a gate, press **Enter** to approve or type feedback; feedback is available to a
`loop_condition` (e.g. `content.get('answer','').strip() not in ('','yes','approved')`).

## Loop conditions & the `{{ idea }}` var

- `loop_condition` and `script` commands are **MiniJinja** (Jinja-compatible).
  Python idioms resolve natively: `.get('k')`, `.get('k', default)`, `.strip()`,
  `.startswith(...)`, `x in (...)`, and the `tojson` filter.
- `content` in a `loop_condition` is the phase's artifact content (a map).
- `{{ idea }}` (the run's idea text) is available in `script` commands.

## Conditional phases (`when`)

A phase with `when` runs only if the expression is true when the phase is
reached. The expression sees `artifacts` — every phase recorded so far, by name,
mapped to its artifact content — and `idea`:

```yaml
- name: review
  type: agent
  tools: files_and_test
- name: fix
  type: agent
  when: "artifacts.review.verdict not in ('approved', 'approved_with_suggestions')"
- name: migrate
  type: script
  command: "make migrate"
  when: "artifacts.plan.get('schema_change')"
- name: notify
  type: script
  command: "echo skipped tests"
  when: "artifacts.test.skipped"       # a skipped phase's content is {skipped: true, reason}
```

- Script artifacts are `{stdout, stderr, exit_code}`, so `"'FAIL' in artifacts.test.stdout"` works.
- A missing phase or field is undefined, which is false, so check spelling:
  `artifacts.review.verdikt != 'approved'` is always true.
- A skipped phase counts as done. Its DAG dependents still run, and the TUI marks it `⊘`.
- Inside a `loop_back_to` range the condition is re-evaluated on every pass.
- A `when` that doesn't parse fails `flows plan` and run start. One that fails while
  being evaluated stops the run; the phase is never silently skipped or run.

---

## Agent prompt templates

An agent's system prompt is a MiniJinja template. Lookup order (first match wins):
`<repo>/.flows/prompts/<template>/<phase>.md` →
`~/.flows/prompts/<template>/<phase>.md` → builtin. `flows templates sync` writes
the builtins to `~/.flows/prompts/` so you can edit them.

**Variables available in a prompt:**

| variable | value |
|---|---|
| `idea` | the run's idea / task text |
| `repo_tree` | gitignore-aware file tree of the repo |
| `phase_name` | the current phase's name |
| `prior_artifacts` | map of `{phase_name: artifact}` for all completed phases |
| `tools_list` | rendered list of the phase's enabled tools |

Same MiniJinja idioms as above. To pass a prior phase's artifact forward cleanly,
render it as JSON: `{{ prior_artifacts['spec'].get('content', {}) | tojson(indent=2) }}`.

**`AGENTS.md`** — the cross-tool convention for repo instructions. If present,
`~/.flows/AGENTS.md` (global) and `<repo>/AGENTS.md` (project, wins) are appended
to **every** agent's system prompt. Use it for house rules like *"run all tooling
through `make`."*
