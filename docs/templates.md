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
flows templates install <git> # install a shared template (see below)
```

- **`sync`** drops editable copies of every shipped template (and the agent
  prompts) into `~/.flows/`. Edit in place; your copy wins over the builtin.
  `--force` overwrites.
- **`new <name>`** writes `~/.flows/templates/<name>.yml` — one of each phase type
  with every common field commented, so you learn the format by editing. Run it
  with `flows run "…" --template <name>`.
- For a **single project**, put a `.flows.yml` at the repo root (`flows init`
  detects your stack and generates one).

### Installing templates from git

```bash
flows templates install https://github.com/acme/flows-templates//ship@v1
flows templates installed               # source, pinned commit, trust
flows templates update ship             # fetch the ref again, show a diff, confirm
flows templates remove ship
flows templates trust ship [--revoke]
```

The source is `<git-url>[//<path>][@<ref>]`. `<path>` is a `.yml` template or a
directory holding exactly one; its prompts come from `prompts/<stem>/*.md` next
to it. The ref (default: the default branch) is resolved to a commit and pinned in
`~/.flows/templates/registry.lock`; the template goes to
`~/.flows/templates/<name>.yml` and its prompts to `~/.flows/prompts/<name>/`.
`--as <name>` renames it. Names of builtins or of your own global templates are
refused, so an install never silently replaces something.

Before writing anything, install (and update) prints a review: the commit, every
phase, every script command, bash allowlists and risky settings
(`unrestricted`, `expose_credentials`), includes and prompt files. Nothing is
installed unless you confirm (`--yes` skips the prompt, not the review).

**Installed templates are untrusted by default.** A template is code, so:

- their **script phases run in the shell sandbox** (writes limited to the
  project, credential stores hidden, environment cleared) instead of directly on
  your machine, and fail if no sandbox is available;
- their agent phases **can't expose credentials** (`expose_credentials` is
  ignored);
- this holds wherever their phases appear, including through `include:`, and
  `flows plan` marks them `(untrusted template: confined)`.

`flows templates trust <name>` (or `install --trust`) lifts both after you've
reviewed it; `update` resets trust unless you pass `--trust` again. A project
template (`.flows/templates/<name>.yml`) with the same name still wins and is
treated like any local template.

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

## Reusing templates (`include`)

An entry `include: <template>` in a phases list (in a template or `.flows.yml`)
is replaced by that template's phases. Templates resolve like any other
(project `.flows/templates/` → `~/.flows/templates/` → builtin); only their
`phases` are used.

```yaml
# .flows/templates/checks.yml
phases:
  - name: lint
    type: script
    command: "{{ vars.lint_cmd }}"
  - name: test
    type: script
    command: make test
    on_failure: retry:lint

# .flows/templates/ship.yml
phases:
  - name: implement
    type: agent
  - include: checks              # → lint, test
    with: { lint_cmd: "make lint" }
  - include: review              # builtin → review, approve
  - include: checks
    as: final                    # → final.lint, final.test
    with: { lint_cmd: "make lint-strict" }
    when: "artifacts.approve.answer == ''"
```

- **Names.** Included phases keep their names; a clash is a load error. `as: x`
  renames them `x.<name>` and rewrites `depends_on`, `loop_back_to` and `retry:`
  among them. Inside an aliased include, prompts and `when:` also see the
  include's artifacts by short name (`artifacts.lint` means `final.lint`). From
  outside, use `artifacts['final.lint']`.
- **`with:`** values appear as `vars` in the included phases' prompts, script
  commands and `when:`. A nested include's values override the outer ones.
- **`when:`** on the include applies to every included phase (ANDed with the
  phase's own `when`).
- **Prompts.** An included phase without `prompt:` uses its own template's
  prompts (`prompts/<included template>/<phase>.md`), so builtin templates bring
  their prompts with them.
- **Loops and the DAG.** Includes are flattened before validation, so the normal
  rules apply. In a DAG pipeline (any `depends_on`), an include may set
  `depends_on:` for its first phases, `depends_on: [final]` (an alias, or the
  template name) means all of that include's phases, and an included template
  without `depends_on` is chained in order. Including a template that uses
  `depends_on` in a linear pipeline is an error.
- Includes nest up to 8 deep; cycles are errors. `flows plan` shows the
  flattened pipeline.

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
| `changes` | uncommitted work: untracked file names, then `git diff HEAD` (≤ 20k chars; empty outside git or on a clean tree) |
| `tools_list` | rendered list of the phase's enabled tools |

Same MiniJinja idioms as above. To pass a prior phase's artifact forward cleanly,
render it as JSON: `{{ prior_artifacts['spec'].get('content', {}) | tojson(indent=2) }}`.

**`AGENTS.md`** — the cross-tool convention for repo instructions. If present,
`~/.flows/AGENTS.md` (global) and `<repo>/AGENTS.md` (project, wins) are appended
to **every** agent's system prompt. Use it for house rules like *"run all tooling
through `make`."*
