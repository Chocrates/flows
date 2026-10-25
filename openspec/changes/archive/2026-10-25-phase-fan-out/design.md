## Context

`run_linear` (`flows-runner/src/runner.rs`) walks a fixed `&[PhaseConfig]` by index; `loop_back_to` / `retry:` jump by `index_of(name)`, loop budgets are keyed by phase name, and every phase's artifact lands in a flat `prior_artifacts` map. The pipeline is fully known at load time: includes (`flows-core/src/include.rs`) are spliced before validation and carry an `IncludeOrigin { template, local_name, prefix, vars }` that already gives us name prefixing, `vars` in prompts/commands/`when:`, and short-name artifact scoping (`scoped_artifacts`).

The `software` template's `design` phase emits `implementation_order: ["Step 1: …"]` as free text; a single `implement` agent (200 rounds, whole spec + design in its prompt) builds everything, and only then do `lint` / `test` run, with `retry:implement` redoing the whole build. There is no run-resume feature today (`run_pipeline`'s `start_phase` is always `None` from the CLI/TUI).

## Goals / Non-Goals

**Goals:**
- Run a group of phases once per item of a list produced by an earlier phase — the general mechanism, not something specific to `implement`.
- Keep each item's prompt small: only its task, the requirements it references, and short summaries of finished items.
- Per-item gates: a failing check retries that item only.
- Builtin `software` / `feature` templates use it for implementation.

**Non-Goals:**
- Concurrent items (shared working tree → write conflicts; would need worktrees).
- Fan-out in DAG pipelines.
- Run resume. We freeze the item list in `state.json` so a later resume change can rebuild the expansion, but no resume entry point is added here.
- Generating literal `openspec/changes/*` files from the run. The structured JSON task list in the design artifact is the source of truth; a script phase can render OpenSpec files from it later.

## Decisions

### 1. Group syntax: a phase entry with `for_each` + nested `phases`

```yaml
- name: implement
  for_each: "artifacts.design.tasks"
  as: task
  max_items: 30
  phases:
    - name: code
      type: agent
      tools: research_and_code
      max_tool_rounds: 60
    - name: check
      type: script
      command: "@auto:test"
      on_failure: retry:code
```

`PhaseConfig` gains `for_each: Option<String>`, `as_var: Option<String>` (`#[serde(rename = "as")]`), `phases: Vec<PhaseConfig>`, `max_items: Option<u32>`. A group is a `PhaseConfig` whose `for_each` is set; its own `phase_type` is ignored (validation rejects `type`/`prompt`/`tools`/`command`/`loop_back_to`/`depends_on` on it).

*Alternative:* a separate `fan_out:` top-level entry kind like `include:`. Rejected — a group needs a name (artifact, `when:`, loop target), so it's a phase.

*Alternative:* `for_each` on a single phase, no nesting. Rejected — the per-item test gate with `retry:code` is the main point; it needs at least two phases per item.

### 2. Expansion by splicing into an owned phase list

`run_linear` switches from `ctx.phases` (borrowed) to a local `Vec<PhaseConfig>`. When `idx` reaches a group:

1. Evaluate `when:`; if false, skip as today.
2. If `state.fan_outs[group]` exists and any item of the group completed → reuse it; else evaluate `for_each` (new `condition::eval_list`, same `when_env` sandbox, sees `artifacts`, `idea`, `vars`) and normalize items (string → `{id, title}`; id = `id` field or position), validate ids, `max_items`, order by item `depends_on` (stable Kahn topo-sort).
3. Write the group artifact `{items, completed: [], failed: [], skipped: []}` and `state.fan_outs[group] = [ids]`.
4. Remove any previously spliced phases of this group, then splice `n × nested` phases directly after the group entry. Each copy is the nested `PhaseConfig` with `name = <group>.<id>.<phase>`, `retry:` / `loop_back_to` rewritten to the same prefix (reuse the include rewrite helper), and `origin = IncludeOrigin { template: <run template>, local_name: <phase>, prefix: "<group>.<id>.", vars: outer_vars ∪ item_vars }` plus a new `fan_out: Option<FanOutItem { group, id }>` field on `IncludeOrigin`.
5. Mark the group itself completed and continue at `idx + 1`.

Because expanded phases are ordinary phases with unique names, everything else is unchanged: `index_of` jumps, per-name `loop_counts` (each item gets its own budget), `persist`, phase-start/complete callbacks, cost guard, `llm.jsonl`. Prompt lookup uses `origin.local_name` under the run's template, so nested `code` finds `software/code.md` the same way included phases already do.

*Alternative:* recursive `run_linear` on a sub-slice per item. Rejected — duplicates cancellation, cost, callbacks and state bookkeeping, and `loop_back_to` from after the group back into it would cross the recursion boundary.

### 3. Item variables

Item vars are computed at splice time except `<as>_done`, which changes as items finish. `<as>_done` is injected at render time: the runner builds it from the group artifact's `completed` list plus each completed item's last agent artifact `content.summary`. Everything flows through the existing `phase.vars()` path used by prompts, script commands and `when:`. Short-name scoping comes free from `scoped_artifacts` because the prefix is `<group>.<id>.`.

The group artifact also rolls up the finished items' `summary` lines and `files_written`, so existing prompts that read `prior_artifacts['implement'].summary` (`verify.md`, `review.md`) work unchanged.

### 4. Failure in a group

Expanded phases follow their own `on_failure`. When the resolved action is "stop" and the group has `on_failure: continue`, the runner instead records the item failed, records every transitive dependent as skipped, and jumps to the first phase of the next runnable item. Otherwise the run fails at the expanded phase. On leaving the last item, the runner fills in the group artifact's `completed` / `failed` / `skipped`.

### 5. Validation (`flows-core/src/pipeline.rs`)

- Group: `for_each` passes `check_when`-style syntax check; nested list non-empty; nested names unique; no nested `for_each`; nested `retry:` / `loop_back_to` name a sibling.
- No group and no outside phase may reference an expanded name (`implement.1.code`) — outside phases can only loop back to the group itself.
- A pipeline with any `depends_on` and any group → error.
- No top-level phase name may start with `<group>.` (expanded names must not collide).

### 6. Callbacks and UI

New `RunnerCallbacks` methods, both default no-op:
- `on_fan_out(group, items: &[(id, title)])` — after expansion, so the TUI can insert item rows under the group row.
- `on_fan_out_item(group, id, title, index, count)` — before each item's first phase; foreground prints `implement 3/7 · 1.3 Add token refresh`, TUI highlights the row.

`meta.json` keeps the unexpanded pipeline; `state.fan_outs` lets `flows attach` / `runs` rebuild the expanded list for display.

### 7. Builtin templates

`software.yml`:

```
requirements → spec → design → approve_design
→ implement (for_each design.tasks: code, check)
→ fix (when: artifacts.verify or artifacts.approve)   # skipped on the first pass
→ lint → test → verify (loop_back_to: fix) → approve (loop_back_to: fix) → commit
```

- `spec.md` gains `requirements: [{id: "REQ-1", text, scenarios: [...]}]` so tasks can reference them.
- `design.md` replaces `implementation_order` with `tasks: [{id, title, requirements, files, depends_on, done_when}]` and guidance: 3–12 tasks, each leaves the build green, ordered foundation first.
- New `software/code.md`: renders `vars.task`, the spec requirements whose id is in `task.requirements`, the design's module/file entries for `task.files`, `vars.task_done` summaries, and the previous `check` output when retrying. Ends with the existing summary-artifact contract.
- New `software/fix.md`: the current `implement.md` feedback blocks (verify `unmet_requirements`, approve answer), applied as targeted fixes.
- `feature.yml` / `plan.md` get the same tasks list and group; `feature/code.md`, `feature/fix.md`.
- The `implement` eval template keeps using `feature/implement.md` unchanged, so evals stay comparable.

## Risks / Trade-offs

- [The model emits a bad task list: too many, too fine, missing ids] → `max_items` cap, id normalization, the prompt asks for 3–12 tasks; `approve_design` shows the list to the human before any code is written.
- [Intermediate builds fail because a later task hasn't landed yet, e.g. a test references unwritten code] → the design prompt requires each task to leave the tree compiling with passing tests; `check` retries hit the same task with the test output. If that proves too strict, nested `check` can be swapped for a build-only command per project.
- [A later task breaks an earlier one] → `check` runs the full test suite, not just new tests; final `lint` / `test` / `verify` still run over the whole tree.
- [Per-item context omits something a task needs] → the agent still has `read_file` / `search_code` and the repo map; `vars.task_done` summaries say where earlier work lives.
- [A splicing bug corrupts indices on loop-back] → expansion is idempotent (remove the old splice by `origin.fan_out.group` and re-splice); mock-provider tests cover re-entry and loop-back across the group.
- [More LLM calls per run (N agents instead of 1)] → each call is far smaller; the cost guard still applies across all items.

## Migration Plan

- Additive config: pipelines without `for_each` behave exactly as before.
- `state.json`: optional `fan_outs` (`skip_serializing_if` empty), so old runs load. Artifacts use the existing naming scheme.
- Builtin templates: unedited synced copies update through the existing builtin-sync mechanism; edited copies keep the old monolithic `implement`.
- Rollback: revert the template change; the runner feature is inert without `for_each`.

## Open Questions

- Should `check` default to `@auto:test` or a cheaper build-only `@auto:build` (new resolver)? This change starts with `@auto:test`.
- Should `verify` eventually map `unmet_requirements` → task ids and rerun only those items (needs "loop back to specific items")? Deferred; `fix` covers it for now.
