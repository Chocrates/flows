## ADDED Requirements

### Requirement: Template includes

A pipeline's phase list (in a template or `.flows.yml`) SHALL accept an entry `include: <template name>` that is replaced by the phases of that template, resolved through the normal template lookup. Includes SHALL nest. A cycle, a nesting depth above 8, an unknown template, or an unknown key on the include entry SHALL be a load error. Only the included template's phases SHALL be used.

#### Scenario: Phases spliced in place

- **WHEN** a pipeline lists phase `plan`, then `include: checks` where `checks` has phases `lint` and `test`, then phase `ship`
- **THEN** the pipeline runs `plan`, `lint`, `test`, `ship` in that order.

#### Scenario: Include cycle

- **WHEN** template `a` includes `b` and `b` includes `a`
- **THEN** loading fails with an error naming the cycle.

### Requirement: Names and aliases

Included phases SHALL keep their names unless the include sets `as: <alias>`, which SHALL rename each included phase to `<alias>.<name>` and rewrite `depends_on`, `loop_back_to` and `retry:` references among the included phases. Duplicate phase names in the flattened pipeline SHALL be a validation error that suggests `as:`. In an aliased include, the artifacts visible to included phases' prompts and `when:` conditions SHALL also be available under their short names.

#### Scenario: Same template twice

- **WHEN** a pipeline includes `checks` twice, the second time with `as: final`
- **THEN** it contains `lint`, `test`, `final.lint` and `final.test`.

#### Scenario: Clash without alias

- **WHEN** a pipeline includes a template whose phase name equals an existing phase's name
- **THEN** loading fails with an error naming the phase and suggesting `as:`.

#### Scenario: Short names inside an alias

- **WHEN** a phase in an include aliased `final` has `when: "artifacts.lint.failed"`
- **THEN** the condition reads the `final.lint` artifact.

### Requirement: Include variables

An include SHALL accept `with:` key/value pairs, exposed to every included phase's prompt, script command and `when:` condition as `vars`. For nested includes the inner include's values SHALL override the outer ones.

#### Scenario: Variable in a script command

- **WHEN** an include sets `with: {target: "make check"}` and an included script phase's command is `{{ vars.target }}`
- **THEN** the phase runs `make check`.

### Requirement: Includes, loops and the DAG

Includes SHALL be expanded before pipeline validation, so `loop_back_to`, `retry:` and `depends_on` follow the normal rules on the flattened pipeline. An include entry MAY set `when:`, which SHALL apply to every included phase (combined with each phase's own condition), and `depends_on:`, which SHALL apply to the include's first phases. In a DAG pipeline, a `depends_on` naming an include's alias (or template name, without an alias) SHALL mean all of that include's phases, and an included template without `depends_on` SHALL be chained in its listed order. Including a template that uses `depends_on` in a pipeline that does not SHALL be an error.

#### Scenario: Depend on an include

- **WHEN** a DAG pipeline includes `checks` and phase `ship` has `depends_on: [checks]`
- **THEN** `ship` waits for `lint` and `test`, and `test` waits for `lint`.

#### Scenario: DAG template in a linear pipeline

- **WHEN** a linear pipeline includes a template whose phases use `depends_on`
- **THEN** loading fails with an error explaining that the including pipeline must use `depends_on`.

### Requirement: Included prompts

An included phase without an explicit `prompt:` SHALL look its prompt up under the template it came from and its name within that template, before the generic per-phase prompts.

#### Scenario: Builtin prompt of an included template

- **WHEN** a custom template includes builtin template `review`
- **THEN** the included phases use the builtin `review` prompts.
