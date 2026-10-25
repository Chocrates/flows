## 1. Config model and parsing (flows-core)

- [x] 1.1 Add `for_each`, `as_var` (`as`), `phases`, `max_items` to `PhaseConfig`, and `fan_out: Option<FanOutItem { group, id }>` to `IncludeOrigin`. Defaults keep existing YAML and serialized meta unchanged. Round-trip tests in `models.rs`
- [x] 1.2 Parse nested `phases:` through the same `parse_phase` path (env expansion, presets). Unit tests for a group parsed from YAML
- [x] 1.3 Add `condition::eval_list(expr, artifacts, idea, vars) -> Result<Vec<Value>, String>` (sandboxed `when_env`) plus a syntax check. Tests: list, non-list, undefined, eval error
- [x] 1.4 Add item normalization + ordering helper (`flows-core/src/fan_out.rs`): string → `{id, title}`, id from field or position, id charset check, duplicate check, `max_items`, stable topo-sort on `depends_on`, cycle/unknown-id errors. Tests cover each spec scenario under "Run-time expansion" and "Item ordering"
- [x] 1.5 Add an expansion helper that builds the expanded `Vec<PhaseConfig>` for a group + ordered items: prefixed names, rewritten sibling `retry:` / `loop_back_to`, `IncludeOrigin` with merged vars (`<as>`, `<as>_index`, `<as>_count`) and `fan_out`. Reuse or extract the reference-rewrite helper from `include.rs`. Unit tests

## 2. Validation

- [x] 2.1 Extend `validate_pipeline` with the group rules: forbidden keys on a group, non-empty nested list, unique nested names, no nested group, sibling-only retry/loop targets, `for_each` syntax, no references to expanded names, no top-level name with a `<group>.` prefix. One test per error
- [x] 2.2 Reject groups in DAG pipelines (any `depends_on`). Test
- [x] 2.3 Make `flows plan` (dry run / cost estimate) handle groups: show the group with its nested phases and "× N items (known at run time)". Test in `dryrun`/`estimate`

## 3. State

- [x] 3.1 Add `fan_outs: BTreeMap<String, Vec<String>>` to `RunState` (`serde(default, skip_serializing_if = "BTreeMap::is_empty")`). Test that old `state.json` without the field still loads

## 4. Runner (flows-runner)

- [x] 4.1 Make `run_linear` own a mutable `Vec<PhaseConfig>`. All index lookups use it. Existing tests still pass
- [x] 4.2 On reaching a group: `when:` skip, evaluate or reuse the frozen items, write the group artifact + `state.fan_outs`, remove any old splice, splice the expanded phases, mark the group completed. Empty list → skipped. Mock-provider tests: three items run in order; empty list skips; non-list fails the run at the group
- [x] 4.3 Inject `<as>_done` at render time from the group artifact + completed items' agent summaries. Test that the prompt for item 2 contains item 1's summary
- [x] 4.4 Short-name artifact scoping inside items via `scoped_artifacts` (prefix `<group>.<id>.`). Test: a `when:` / prompt in item 2 reads its own `check`
- [x] 4.5 Item-scoped retry/loop: test that `implement.2.check` failing with `retry:code` reruns only item 2, with a separate loop budget per item
- [x] 4.6 Group `on_failure: continue`: record failed item, skip transitive dependents, continue; fill `completed`/`failed`/`skipped` in the group artifact. Tests for stop and continue scenarios
- [x] 4.7 Re-entry: a later phase looping back to the group reuses the frozen items once any item completed, and re-evaluates if none did. Test both cases
- [x] 4.8 Add `RunnerCallbacks::on_fan_out` and `on_fan_out_item` (default no-op) and fire them. Test with a recording callback

## 5. CLI / TUI output

- [x] 5.1 Foreground + background worker: print `<group> <i>/<n> · <id> <title>` on `on_fan_out_item`
- [x] 5.2 TUI live run + attach view: insert item rows under the group row on `on_fan_out`; highlight the current item; rebuild from `state.fan_outs` on attach. pty/vt100 test showing the item rows

## 6. Builtin templates and prompts

- [x] 6.1 `software/spec.md`: emit `requirements: [{id, text, scenarios}]` with stable ids
- [x] 6.2 `software/design.md`: replace `implementation_order` with `tasks: [{id, title, requirements, files, depends_on, done_when}]`. Guidance: 3–12 tasks, each leaves the build green
- [x] 6.3 New `software/code.md` (one task, referenced requirements only, done summaries, previous check output on retry) and `software/fix.md` (verify/approve feedback). Update `verify.md` to read the per-item summaries
- [x] 6.4 Rewrite `software.yml`: `implement` group (`code`, `check: @auto:test, retry:code`), `fix` with `when`, `verify`/`approve` loop back to `fix`. Template-loads-and-validates test plus a mock-provider end-to-end run over a 2-task design
- [x] 6.5 Same for `feature`: `plan.md` tasks list, `feature/code.md`, `feature/fix.md`, `feature.yml`. Keep `feature/implement.md` (used by the `implement` eval template)
- [x] 6.6 Check that the builtin-sync logic updates unedited copies and leaves edited ones alone with the new template shape (existing sync tests + one new case)

## 7. Docs and gate

- [x] 7.1 Document `for_each` groups in `docs/templates.md` / `docs/configuration.md`, and update the `software`/`feature` descriptions in README
- [x] 7.2 `make check` green
