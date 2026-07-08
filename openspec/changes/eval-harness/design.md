## Context

`crates/flows-cli/src/eval.rs` already runs a fixture headless: `materialize` the
repo into a tempdir + `git init`, resolve phases, `run_pipeline` with an
auto-proceed `EvalCallbacks`, run the shell `verify`, pull tokens/cost/peak/rounds
from the run's `llm.jsonl` via `parse_usage`, then `delete_run`. This change keeps
that core and grows a matrix, a store, suites, and manual grading around it.

`ContextConfig` (models.rs) already exists but only holds `exclude` globs. The
`models:` profile map + `resolve_llm` / `merge_profiles` (config.rs) already exist
from `multi-model-routing`. `preflight_llm` and `default_provider_factory` exist.

## Goals / Non-Goals

**Goals:** by-model comparison with stored history; size tiers; selectable context
configs (flags plumbed, features stubbed); coder/reasoner suites with different
success sources; deferred manual validation; an offline comparison report.

**Non-Goals:** the context features themselves; deterministic retrieval eval
(needs them); LLM-judge grading; HTML/CSV viz; changing on-disk run formats.

## Decisions

**D1 — One matrix command, in-process override.** `flows eval run` iterates
`model × config × fixture(filtered by suite/size) × trial`. For a cell it clones
the resolved global/project config, pins the base `llm` to the resolved model and
clears every `phase.model` (so the whole pipeline runs on that one model — eval
measures the model, not profile routing), and sets `project.context` to the named
config. No new `run_pipeline` parameter — the override rides the configs it already
takes. Rationale: reuses the entire existing run path; the model/config axes are
pure config substitution.

**D2 — Suite picks the success source.** `suite: coder` → the shell `verify` exit
code is the pass signal (manual optional). `suite: reasoner` → there is no
meaningful shell verifier for a design/plan; the human grade against `rubric` is
the pass signal. This is the concrete coder/reasoner difference and why reasoner
fixtures require a `rubric` and coder fixtures require a `verify`.

**D3 — JSONL append store, latest-record-per-id.** `~/.flows/eval/results.jsonl`
is append-only; each trial appends one record, and a later manual grade appends a
*new* record with the same `trial` id. Readers fold by id taking the last record,
so grades overlay metrics without mutation. Rationale: crash-safe, greppable, no
new dependency, trivially accumulates history for cross-model comparison. Trial
outputs (the `git diff` + task/rubric meta) are saved under `trials/<id>/` for
`review` — the execution tempdir is still discarded.

**D4 — Context flags on the existing `ContextConfig`, stubbed.** Add
`search_tool`, `range_reads`, `slim_tree`, `repo_map`, `budgeting`, `retrieval`
bools (default false = baseline) to `ContextConfig`; parse + merge them. A
`parse_context_config(name)` maps a config name (`baseline`, `all`, `+search`,
`+range`, `+slim`, `+map`, `+budget`, `+rag`, combinable) to a `ContextConfig`.
No runner/tool reads the flags yet — that is `context-delivery-for-small-models`.
This change only makes the flags configurable + selectable so the eval matrix has
a real config axis and an "all off == baseline" guarantee to test against.

**D5 — Size is an explicit fixture tag.** `size: small|medium|large` is authored,
not auto-measured from repo tokens. Cheap, intentional, and lets a deliberately
tiny fixture stress a large window. `--size` filters; the report breaks out by tier.

**D6 — Terminal-table report only.** `flows eval report` reads the store and prints
a table grouped by `(model, config, suite, size)` with mean ± spread over trials.
No HTML/CSV. The JSONL store stays a clean export point if richer viz is wanted.

## Risks / Trade-offs

- Selectable configs that don't yet change behavior can mislead ("why is +search
  the same as baseline?"). Mitigation: until the features land, all configs
  produce identical runs by design; the report footnote / docs note this, and the
  baseline-all-off test pins the contract.
- Manual review is unattended-deferred, so a matrix can finish with reasoner cells
  ungraded. The report shows an explicit ungraded count rather than scoring them.

## Migration

Additive. Existing `hello-file` fixture keeps working (defaults: `suite: coder`,
`size: medium`). Old `flows eval` (flat) becomes `flows eval run`. New store dir is
created on first run.
