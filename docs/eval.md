# Benchmarking your model (`flows eval`)

Small local models vary a lot in how well they drive an agent loop. `flows eval`
measures it as a **matrix**: pick models, context configurations, and fixtures;
each `(model × config × fixture × trial)` cell runs headless, and every result is
stored so you can compare models over time.

```bash
# Run a matrix and store the results:
flows eval run --models reasoner,coder --configs baseline,+search --suite coder
flows eval run --models qwen2.5-coder --size small --trials 3   # 3 trials/cell
flows eval run --fixtures ./my-fx                               # your own fixtures

flows eval list                     # show discovered fixtures (filter --suite/--size)
flows eval review                   # grade ungraded trials (reasoner pass signal)
flows eval report                   # comparison table (filter --model/--config/--suite)
flows eval clean                    # delete stored results (no filters = wipe all)
flows eval retrieval                # deterministic recall@k / MRR of the rankers (no LLM)
```

`flows eval clean` removes trials from the store — filter with `--model`,
`--fixture`, `--suite`, `--granularity` (no filters wipes everything). It prompts
for confirmation unless `--yes`.

- **`--models`** — CSV of model-profile names (from your `models:` map) or literal
  model strings. Each model is forced across the whole pipeline. Omit → your
  configured default. Models failing preflight are skipped, not fatal.
- **Pairing models to suites** — a model runs only its suites' fixtures when
  restricted. Tag the profile once in config so it auto-pairs:
  ```yaml
  models:
    coder:    { model: coder,    api_base: …, suites: [coder] }
    reasoner: { model: reasoner, api_base: …, suites: [reasoner] }
  ```
  Then `flows eval run --models coder,reasoner` runs coder on coder fixtures and
  reasoner on reasoner fixtures — not the full cross-product. Override for one run
  by appending `:coder` / `:reasoner` to a model.
- **`--configs`** — CSV of context configurations: `baseline` (all off), `all`, or
  combined feature tokens joined with `+`: `search`, `range`, `slim`, `map`,
  `budget`, `rag` (e.g. `search+map`), plus the eval-only `oracle` (injects a
  fixture's labeled `relevant` files as an upper-bound reference). A fixture can
  declare its own `configs:` axis to A/B automatically. See
  [context delivery](./configuration.md#context-delivery-for-small-models).
- **`flows eval retrieval`** — a **deterministic** ranking eval (no model call):
  for every fixture with labeled `relevant` files, it ranks files by the repo map
  and by embedding retrieval and reports recall@1/3/5 + MRR. `--corpus DIR` adds
  retrieval corpora (see below); `--per-query` prints a row per query.

### Retrieval corpora

A corpus is one repository with many labeled queries — cheaper than a fixture per
query (one repo copy, one index) and the right shape for ranking work:

```
eval-fixtures/retrieval/<name>/
  queries.yml     # queries: [{task, relevant: [repo-relative paths]}]; optional repo: <path>
  repo/           # source tree (or reuse another fixture's via `repo: ../../x/repo`)
```

`eval run` ignores corpus folders. Every `relevant` path must exist, or the eval
stops. The shipped corpus is **path-free**: no query names a file, directory, or
identifier from its repo, so rankers can't win by matching paths.

| corpus | repo | queries |
|---|---|---|
| `flows-workspace` | this workspace (snapshot shared with `whole-workspace`) | 14 |
| `py-invoicing` | synthetic Python billing backend, 29 files | 18 |
| `ts-kanban` | synthetic TypeScript kanban app (client + server), 31 files | 18 |
| `go-jobqueue` | synthetic Go job-queue service, 30 files | 18 |

Baseline (default `all-MiniLM-L6-v2`), `flows eval retrieval --corpus eval-fixtures/retrieval`:

```
fixture / corpus             ranker    r@1    r@3    r@5    MRR    n
flows-workspace              map      0.00   0.11   0.11   0.11   14
flows-workspace              rag      0.21   0.36   0.57   0.46   14
go-jobqueue                  map      0.17   0.22   0.42   0.35   18
go-jobqueue                  rag      0.33   0.67   0.81   0.73   18
py-invoicing                 map      0.22   0.58   0.64   0.49   18
py-invoicing                 rag      0.69   0.83   0.83   0.84   18
ts-kanban                    map      0.08   0.26   0.39   0.36   18
ts-kanban                    rag      0.28   0.47   0.57   0.53   18
MEAN                         map      0.12   0.30   0.40   0.34   68
MEAN                         rag      0.39   0.60   0.70   0.65   68
```

On the older per-task fixtures (most name their target file) the same rankers
score MRR 0.47 / 1.00; judge ranking changes on the corpus.

```
model            config     suite    size        pass    in_tok      peak_ctx     cost  rounds   n
qwen2.5-coder    baseline   coder    small   100% 3/3      3.1k        3.4k±0.2k  $0.00     3.0   3
qwen2.5-coder    +search    coder    small   100% 3/3      2.4k        2.6k±0.1k  $0.00     2.3   3
claude-opus-4-8  baseline   coder    medium  100% 3/3     12.0k       13.1k±0.4k  $0.14     5.0   3
```

## Fixtures

A fixture is a folder with `fixture.yml` and (for phase/flow) a `repo/` subtree:

```yaml
suite: coder            # coder | reasoner
size:  small            # small | medium | large
granularity: phase      # flow | phase | turn
task:  "…"
template: blank
verify: "cargo test"    # required for coder: exit 0 == success
rubric: |               # required for reasoner: shown during `eval review`
  - correct root cause?
  - fix actionable / idiomatic?
relevant:               # labels the files/symbols a good run should surface
  - src/calc.sh
timeout_secs: 600
```

**`relevant:`** labels the files/symbols a good run should surface — the target for
the context-delivery + retrieval evals. It is **not** injected: eval trials run the
real flow prompts unmodified, so a read-incapable model's failure to ground is a
genuine **negative baseline** to measure a context feature against.

### Granularity

Picks how much of the pipeline a fixture exercises — the report groups by it so
levels are never compared:

- `flow` — the whole pipeline (a multi-phase `template`). Realistic end-to-end, but
  high variance.
- `phase` — a **single agent phase** (a one-agent-phase `template` like `blank`).
  The clean A/B unit for context features. Rejected if the template has more than
  one agent phase.
- `turn` — a **single model turn**: one prompt → one completion, no tool loop, no
  repo. Cheapest, lowest-variance. Coder turns auto-grade by `expect`; reasoner
  turns are graded manually.

Phase and flow fixtures run the **real default prompts**. A `turn` fixture is just
a prompt + expectations (no `repo/`, no `verify`):

```yaml
suite: coder
granularity: turn
system: "You are a terse coding assistant. Output only code."   # optional
task: "Write a Rust fn `is_prime(n: u64) -> bool`."
expect: ["fn is_prime", "u64"]     # completion must contain all of these
max_tokens: 512                    # optional (default 2048)
```

- **coder** fixtures are scored by the shell `verify` command (exit 0 = pass).
- **reasoner** fixtures have no shell check — success is a **manual grade** via
  `flows eval review`. Ungraded reasoner trials show as `ungraded` in the report.

## Reviewing

`flows eval review` opens a **full-screen TUI**: the trial's **transcript** (the
context the model saw + its response) scrolls in the top pane while you rate in the
bottom pane. Scroll with ↑/↓, PgUp/PgDn, `g`/`G`, or the wheel; `1`–`5` rates the
current dimension (auto-advances), `Tab`/←/→ moves between dimensions, `n` for a
note, `Enter` saves + next, `q` quits. (Non-terminal → line prompts.) Every trial
is rated **1–5 on five dimensions**:

| dimension | question |
|---|---|
| **Correctness** | is the output actually right? (pass/fail derives from this, ≥3 = pass) |
| **Efficiency** | did it get there without wasted rounds/tokens? |
| **Tool use** | right tools, valid args, recovers from errors? (`n/a` for `turn`) |
| **Grounding** | used the real repo/context vs hallucinating? |
| **Instruction following** | obeyed task constraints + output format? |

`flows eval report` shows per-dimension means as a `C/E/T/G/I` column.

## Storage

Results persist under `~/.flows/eval/` (an append-only `results.jsonl` plus a
`trials/<id>/` dir per trial). `flows eval report` reads that store — no model
calls — so comparison is offline and accumulates across runs. Each trial dir holds:

- `output.md` — the response (turn) or produced artifacts + repo diff (phase/flow),
- `transcript.txt` — the exact context the model saw and its response,
- `run-log.jsonl` — for phase/flow, the per-round token/cost/latency trace.

The transcript is written by the agent loop when `FLOWS_TRANSCRIPT` is set (the
harness sets it automatically). You can set `FLOWS_TRANSCRIPT=1` yourself to capture
any run's prompts to `~/.flows/runs/<id>/transcript.txt`.
