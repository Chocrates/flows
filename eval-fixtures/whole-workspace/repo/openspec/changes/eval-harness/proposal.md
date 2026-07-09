## Why

`flows` targets small / local models (llama.cpp) on real codebases, and we are
about to add context-delivery features (search, repo map, budgeting, retrieval)
whose whole justification is "make a small model succeed on a big repo." We need
a measurement rig to prove each feature earns its tokens — and to tell which model
is worth running where.

Today `flows eval` is a thin baseline: one model, one flat corpus, a shell
verifier pass/fail, a single-config table printed and thrown away. It cannot
answer the questions we actually have: how does *this* model compare to *that*
one, does the repo fit a small window, which context feature helped, and did a
reasoning model actually design well (shell verify can't judge prose).

## What Changes

- **Model axis.** `flows eval run --models <csv>` evaluates a set of models
  (profile names from the `models:` map or literal model strings), forcing each
  model across the whole pipeline for its cells.
- **Context-config axis.** `--configs <csv>` selects named context configurations
  (`baseline`, `+search`, `+search+map`, `all`, …). This change also lands the
  `context.*` feature-flag plumbing (config block + named-config parser); the
  features themselves stay stubbed until `context-delivery-for-small-models`.
- **Size tiers.** Fixtures are tagged `size: small | medium | large`; `--size`
  filters, and the report breaks results out by tier.
- **Coder vs reasoner suites.** Fixtures are tagged `suite: coder | reasoner`. The
  suite picks the success signal: coder → shell `verify`, reasoner → manual grade
  against a `rubric`. `--suite` filters.
- **Persistent results store.** Every trial is appended to a JSONL store under
  `~/.flows/eval/`, with its diff/output saved for later grading, so runs
  accumulate and models are compared over time.
- **Manual validation.** `flows eval review` walks ungraded trials, shows the task
  / rubric / diff, and records a human pass/fail + score + note.
- **Comparison report.** `flows eval report` reads the store (no model calls) and
  prints a per-`(model, config, suite, size)` table with mean ± spread across
  trials.

Non-goals: the actual context features (search_code, repo map, budgeting,
retrieval) — this change only makes their flags selectable; the deterministic
retrieval eval (recall@k / MRR) stays in `context-delivery-for-small-models` since
it needs those features; LLM-as-judge grading; HTML/CSV export (terminal only).

## Capabilities

### New Capabilities
- `eval-harness`: a model × context-config × suite × size evaluation matrix over
  task fixtures, a persistent results store for cross-model comparison, deferred
  manual validation for reasoning tasks, and a comparison report — plus the
  `context.*` feature-flag configuration and named-config selection the matrix
  needs.

### Modified Capabilities
<!-- Supersedes the end-to-end pieces of context-delivery-for-small-models §5
(5.1 fixtures format, 5.3 headless runner, 5.4 report). That change's §5.2
deterministic retrieval eval is unaffected and still owned there. No security
invariants change: fixtures run in the same sandbox/verify path as a normal run. -->

## Impact

- **Code**: `flows-core` (`ContextConfig` feature flags, config parse/merge, a
  named-config parser); `flows-cli` (`eval.rs` fixture v2 + matrix runner, new
  `eval_store.rs`, `main.rs` `eval` subcommand group run/list/review/report).
- **Assets**: suite/size-tagged starter fixtures under `flows-cli/assets/eval/fixtures`.
- **On-disk**: new `~/.flows/eval/` store (JSONL + per-trial output dirs) — additive.
- **Docs**: README `flows eval` section.
