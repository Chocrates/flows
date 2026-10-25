# phase-fan-out Specification

## Purpose
Run a group of phases once per item of a list produced earlier in the run (e.g. the tasks of a generated design), so implementation proceeds step by step with per-item context, per-item retries and an item-level failure policy.
## Requirements
### Requirement: Fan-out group definition

A phase entry SHALL be a fan-out group when it sets `for_each: <expression>`. A group SHALL also set `name`, a nested `phases:` list of one or more phases, and MAY set `as: <var>` (default `item`), `when:`, `on_failure:` and `max_items:` (default 50). A group SHALL NOT set `type`, `prompt`, `tools`, `command`, `loop_back_to` or `depends_on`. Nested phases SHALL NOT themselves be fan-out groups. A `for_each` expression SHALL be syntax-checked at validation. A pipeline that uses `depends_on` (DAG mode) SHALL reject fan-out groups.

#### Scenario: Valid group

- **WHEN** a pipeline has phase `design`, then a group `implement` with `for_each: "artifacts.design.tasks"`, `as: task`, and nested phases `code` (agent) and `check` (script)
- **THEN** the pipeline validates.

#### Scenario: Nested group

- **WHEN** a group's nested `phases:` contains another phase with `for_each`
- **THEN** validation fails with an error naming both groups.

#### Scenario: Group in a DAG pipeline

- **WHEN** a pipeline where any phase sets `depends_on` contains a fan-out group
- **THEN** validation fails with an error saying fan-out requires a linear pipeline.

### Requirement: Run-time expansion

When the runner reaches a fan-out group whose `when:` is true, it SHALL evaluate `for_each` against the recorded artifacts (`artifacts.<phase>`), `idea` and `vars`, in the same sandboxed environment as `when:`. The result SHALL be a list; each item SHALL be an object or a string (a string `s` is treated as `{"id": "<n>", "title": s}`). Each item's id SHALL be its `id` field if present, else its 1-based position. The runner SHALL then run the nested phases once per item, in order, under the names `<group>.<id>.<phase>`. A non-list result, an evaluation error, a duplicate id, an id that is not a non-empty string of letters, digits, `.`, `-` or `_`, or more than `max_items` items SHALL fail the run at the group. An empty list SHALL record the group as skipped.

#### Scenario: Three tasks

- **WHEN** `artifacts.design.tasks` is `[{"id":"1"},{"id":"2"},{"id":"3"}]` and the group `implement` has nested phases `code`, `check`
- **THEN** the runner runs `implement.1.code`, `implement.1.check`, `implement.2.code`, `implement.2.check`, `implement.3.code`, `implement.3.check`, then the phase after the group.

#### Scenario: Not a list

- **WHEN** the `for_each` expression evaluates to a string or an undefined value
- **THEN** the run fails at the group with an error naming the expression.

#### Scenario: Too many items

- **WHEN** the expression yields 80 items and `max_items` is 50
- **THEN** the run fails at the group before any item runs.

#### Scenario: Empty list

- **WHEN** the expression yields `[]`
- **THEN** the group is recorded as skipped and the run continues with the next phase.

### Requirement: Item ordering

When items carry `depends_on` (a list of other item ids), the runner SHALL run items in a dependency-respecting order, keeping the listed order among items that are otherwise unordered. A `depends_on` naming an unknown id, or a dependency cycle, SHALL fail the run at the group. Items SHALL run one at a time.

#### Scenario: Dependency reorders

- **WHEN** items are listed `A` (depends_on `[B]`), `B`, `C`
- **THEN** they run in the order `B`, `A`, `C`.

#### Scenario: Cycle

- **WHEN** item `A` depends on `B` and `B` depends on `A`
- **THEN** the run fails at the group with an error naming the cycle.

### Requirement: Item variables and context

Every expanded phase SHALL see, in its prompt, script command and `when:` condition, `vars.<as>` (the item), `vars.<as>_index` (1-based position in run order), `vars.<as>_count` (number of items), and `vars.<as>_done` (a list of `{id, title, summary}` for items already finished in this group, where `summary` is the item's last agent phase's `content.summary` if any). Variables from an enclosing include SHALL also be visible. Within an item, artifacts of that item's phases SHALL also be visible under their short (unprefixed) names, overriding any other phase of the same short name.

#### Scenario: Prompt sees its task

- **WHEN** item `2` with title `"Add parser"` runs nested phase `code` whose prompt renders `{{ vars.task.title }} ({{ vars.task_index }}/{{ vars.task_count }})`
- **THEN** the rendered prompt contains `Add parser (2/3)`.

#### Scenario: Short name inside an item

- **WHEN** nested phase `code` of item `2` reads `artifacts.check` (or `prior_artifacts['check']` in a prompt)
- **THEN** it gets `implement.2.check`, not `implement.1.check`.

### Requirement: Item-scoped retries and loops

A nested phase's `retry:<phase>` and `loop_back_to: <phase>` SHALL name a sibling nested phase and SHALL jump to that phase of the same item. A target outside the group SHALL be a validation error. Loop iteration counts SHALL be tracked per expanded phase, so each item gets its own `max_loop_iterations` budget.

#### Scenario: Test failure retries one step

- **WHEN** `implement.2.check` fails with `on_failure: retry:code`
- **THEN** the runner reruns `implement.2.code` then `implement.2.check`, and does not rerun item `1`.

#### Scenario: Target outside the group

- **WHEN** a nested phase sets `on_failure: retry:design` where `design` is outside the group
- **THEN** validation fails naming the phase and the target.

### Requirement: Group failure handling

When an expanded phase fails with `on_failure: stop` (or exhausts its retries/loops and stops), the run SHALL fail at that expanded phase. If the group sets `on_failure: continue`, a failed item SHALL be recorded as failed and the group SHALL proceed to the next item whose dependencies did not fail; items depending on a failed item SHALL be recorded as skipped.

#### Scenario: Stop on a failing item

- **WHEN** `implement.2.code` fails with `on_failure: stop`
- **THEN** the run fails with phase `implement.2.code` and item `3` does not run.

#### Scenario: Continue past a failed item

- **WHEN** the group sets `on_failure: continue`, item `2` fails, and item `3` depends on `2` while item `4` does not
- **THEN** item `3` is skipped, item `4` runs, and the group artifact lists `2` as failed and `3` as skipped.

### Requirement: Group artifact and frozen items

At expansion the runner SHALL write the group's own artifact (`<group>`) with `content.items` (the item list in run order), and as items finish SHALL keep `content.completed`, `content.failed` and `content.skipped` (lists of ids) current, together with `content.summary` (one `[<id> <title>] <summary>` line per finished item) and `content.files_written` (the union of the items' `files_written`), so prompts written for a single implement phase keep working. The item list SHALL be frozen for the life of the run: if a later `loop_back_to` re-enters the group, the same items SHALL be used unless no item of the group had completed, in which case the expression SHALL be re-evaluated. The frozen list SHALL also be recorded in `state.json` under `fan_outs.<group>`.

#### Scenario: Re-entry keeps the items

- **WHEN** a phase after the group loops back to the group after items `1`–`3` completed, and the source artifact has since changed to four tasks
- **THEN** the group runs the original three items again.

### Requirement: Progress reporting

The runner SHALL report item progress through `RunnerCallbacks` (an event carrying group name, item id, item title, 1-based index and count) before each item starts. Expanded phases SHALL be reported through the normal phase-start/complete events under their expanded names.

#### Scenario: Progress event

- **WHEN** item `1.3` titled `"Add token refresh"` starts as the 3rd of 7
- **THEN** the callbacks receive a fan-out item event with group `implement`, id `1.3`, title `Add token refresh`, index 3, count 7.

### Requirement: Builtin spec-driven templates use fan-out

The builtin `software` template's `design` phase SHALL emit `content.tasks`, a list of `{id, title, requirements, files, depends_on, done_when}`, and its spec phase SHALL give each requirement an id that tasks reference. Its implementation SHALL be a fan-out group over `artifacts.design.tasks` whose nested phases implement one task and run the project's tests, retrying the task on failure. `verify` and the final `approve` SHALL loop back to a single `fix` phase rather than to the group. The builtin `feature` template SHALL do the same with its `plan` phase.

#### Scenario: Software template expands design tasks

- **WHEN** the `software` template runs with a design artifact containing four tasks
- **THEN** the run contains four implementation items, each with its own test gate, followed by `lint`, `test` and `verify`.

#### Scenario: Verify failure does not rerun steps

- **WHEN** `verify` returns `needs_fixes`
- **THEN** the runner runs `fix` and then the phases after it, not the fan-out items.

