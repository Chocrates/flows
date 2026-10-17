## Why

Templates can't reuse each other. A test-and-fix loop or a review stage gets copied into every template that wants it, and the copies drift.

## What Changes

- A phases-list entry `- include: <template>` splices that template's phases in place. Templates resolve through the usual lookup (project → `~/.flows` → builtin). Includes nest; cycles and depth > 8 are errors. Only the included template's `phases` are used.
- Included phases keep their names; a name clash is a validation error. `as: <alias>` prefixes every included phase as `<alias>.<name>`, and rewrites references inside the include (`depends_on`, `loop_back_to`, `retry:`). Inside an aliased include, prompts and `when:` conditions can still use the short names.
- `with: {key: value}` exposes values to the included phases' prompts, script commands and `when:` conditions as `vars`.
- Optional `when:` on the include entry applies to every included phase; optional `depends_on:` applies to the include's first phases.
- Pipelines are flattened before validation, so loops, retries and the DAG follow the normal rules. In a DAG pipeline, `depends_on: [<alias or template>]` means after all of that include's phases, and an included linear template is chained in order. Including a DAG template in a linear pipeline is an error.
- Included phases find their prompts under the template they came from (`prompts/<included template>/<phase>.md`).

## Capabilities

### New Capabilities
- `pipeline-composition`: including one template's phases in another pipeline.

## Impact

`flows-core` (new `include.rs`, `PhaseConfig.origin`, template + project config loading, `condition::eval_when`), `flows-runner` (prompt lookup, prompt/command/`when` contexts), docs/templates.md, ROADMAP.
