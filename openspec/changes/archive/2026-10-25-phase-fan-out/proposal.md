## Why

Implementation runs as one monolithic `implement` agent phase: a single 200-round tool loop that receives the whole spec and design, builds everything, and is only checked by lint/test at the very end. A failure anywhere reruns the entire build (`retry:implement`), and small/local models drown in context. The design phase already produces an ordered `implementation_order`, but it is free text the runner cannot act on. Implementation should run as N steps — one per task in the generated spec/design — each with focused context and its own build/test gate.

## What Changes

- New `for_each` phase group: a phase entry with `for_each: <expr>`, `as: <var>`, and a nested `phases:` list. When reached at run time, the expression is evaluated against prior artifacts to a list; the nested phases are expanded once per item (named `<group>.<task-id>.<phase>`) and run in order.
- Each expanded phase sees the current item as `vars.<as>` (plus `vars.<as>_index`, `vars.<as>_count`, and summaries of items already done) in prompts, commands and `when:`.
- `retry:` and `loop_back_to` inside the group target phases of the same item only — a failing test reruns that step, not the whole build.
- The item list is frozen into the group's own artifact (and `state.json`) at expansion, so loop-backs re-run the same items and a future resume feature has what it needs.
- Items may declare `depends_on` (task ids); items run sequentially in a dependency-respecting order (no concurrency in this change).
- Builtin templates `software` and `feature`: the design/plan phase emits a structured `tasks` list (id, title, requirement refs, files, depends_on, done_when) instead of `implementation_order`; `implement` becomes a `for_each` group (per-step code agent + test gate); `verify` / `approve` loop back to a new single `fix` phase instead of re-running every step. Spec phase gains requirement ids so tasks can reference them.
- TUI/foreground output shows step progress (`implement 3/7 · 1.3 <title>`).
- **BREAKING (template content only)**: synced builtin copies of `software`/`feature` change shape; edited user copies are left untouched by the existing sync rules.

## Capabilities

### New Capabilities
- `phase-fan-out`: `for_each` phase groups — runtime expansion over an artifact-derived list, per-item variables, item-scoped retry/loops, frozen item list, ordering by item `depends_on`, progress reporting.

### Modified Capabilities
None. Fan-out reuses the include machinery (prefixing, `vars`, short-name artifact scoping) but adds no requirement to `pipeline-composition`; its naming and scoping rules are specified in `phase-fan-out`.

## Impact

- `flows-core`: `PhaseConfig` (`for_each`, `as`, nested `phases`), parsing + validation (`config.rs`, `include.rs` reuse for prefix/vars), `RunState` (expanded groups).
- `flows-runner`: `run_linear` must handle a pipeline that grows at run time; prompt/command rendering exposes per-item vars; new `RunnerCallbacks::on_fan_out` progress event.
- `flows-tui`: step progress display.
- Builtin templates/prompts: `software/{spec,design,implement,verify}.md`, `feature/{plan,implement}.md`, new `fix.md`; templates `software.yml`, `feature.yml`.
- **On-disk format**: `state.json` gains an optional `fan_outs` map (group → frozen item ids); artifacts appear under expanded names (`artifacts/implement.1.2.code.yml`). Both additive; old runs still load.
- **Security**: no change to sandboxing, path scoping or SSRF guards. Expanded phases inherit the group's tools and the `untrusted` flag; the `for_each` expression is evaluated in the same sandboxed MiniJinja environment as `when:`/loop conditions. Expansion is bounded (max items) to keep a hostile/broken artifact from producing an unbounded pipeline.
- DAG pipelines: `for_each` groups are rejected in DAG mode for this change.
