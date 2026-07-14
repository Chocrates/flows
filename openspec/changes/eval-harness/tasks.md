## 1. Context feature flags (flows-core)

- [x] 1.1 Extend `ContextConfig` (models.rs) with `search_tool`, `range_reads`, `slim_tree`, `repo_map`, `budgeting`, `retrieval` bools, all `#[serde(default)]` = false. Keep existing `exclude`.
- [x] 1.2 Parse the new flags in `config.rs` (extend the existing `context` block parse) and merge global→project (project wins). Round-trip test.
- [x] 1.3 Add `parse_context_config(name) -> Result<ContextConfig, String>`: grammar `baseline` (all off), `all` (all on), `+search|+range|+slim|+map|+budget|+rag` combinable; unknown token errors. Tests: baseline all-off, `+search+map` exactly those, unknown errs.

## 2. Fixture format v2 (flows-cli/eval.rs)

- [x] 2.1 Add `Suite` (coder|reasoner) and `Size` (small|medium|large) enums (serde, reject unknown). Extend `FixtureSpec` with `suite`, `size`, `rubric: Option<String>`, `relevant: Vec<String>` (all defaulted).
- [x] 2.2 Validate on load: coder requires `verify`; reasoner requires `rubric`. Error names the fixture. Tests for both + unknown suite/size rejection.
- [x] 2.3 Add suite/size-tagged starter fixtures under `assets/eval/fixtures/` (keep `hello-file` as coder/medium; add a coder/small and a reasoner/small). `--suite`/`--size` filters in `list`/`run`.

## 3. Results store (flows-cli/eval_store.rs)

- [x] 3.1 Define a `TrialRecord` (trial id, ts, model, config, fixture, suite, size, verify_pass, manual_pass, manual_score, note, metrics). Append as JSONL to `~/.flows/eval/results.jsonl`.
- [x] 3.2 Save each trial's output (`git diff` patch + task/rubric meta) under `~/.flows/eval/trials/<id>/` for review.
- [x] 3.3 Reader folds records by trial id taking the latest (grade overlays metrics). Round-trip tests: append trial → read back; append grade → latest wins.

## 4. Matrix runner + CLI group (flows-cli)

- [x] 4.1 Replace `Command::Eval` with an `eval` subcommand group in `main.rs`: `run` (--models,--configs,--suite,--size,--fixtures,--trials), `list`, `review`, `report`.
- [x] 4.2 `run`: for each model, resolve via `merge_profiles`+`resolve_llm`, preflight (skip+report failures). For each (model, config, fixture, trial): clone config, pin base `llm` to the model, clear every `phase.model`, set `project.context = parse_context_config(config)`; run the fixture; append a `TrialRecord`; save output.
- [x] 4.3 Keep the existing `materialize` + git-init, `verify`, `EvalCallbacks`, and `parse_usage` metric extraction.

## 5. Manual review + report (flows-cli)

- [x] 5.1 `review`: iterate store trials with `manual_pass == None` (filterable by suite/size/model); print task + rubric + saved diff; prompt `[p]ass/[f]ail/[s]kip`, optional score 0-5 + note; append a grade record.
- [x] 5.2 `report`: read store, fold by trial id; success source per suite (coder→verify_pass, reasoner→manual_pass, None→ungraded count); print table grouped by (model, config, suite, size) with mean ± spread; `--model`/`--config` filters; no model calls.
- [x] 5.3 Tests: report aggregation (mean + spread) and success-source-by-suite on synthetic records.

## 7. Evaluation granularity

- [x] 7.1 Add a `Granularity` enum (flow|phase|turn, default flow) + fixture `granularity` field; `turn` reserved (errors as not-yet-supported).
- [x] 7.2 Resolve each fixture's phases once per run and validate: `phase` requires exactly one agent phase. Invalid fixtures skipped with a message.
- [x] 7.3 Record granularity per trial; make it part of the report grouping key so levels never mix; add a `--granularity` filter to run/list/report. Tests.
- [x] 7.4 Tag all fixtures (blank template → phase, feature template → flow); add `whole-workspace-phase` (blank/phase) for a flow-vs-phase A/B on the same task.
- [x] 7.5 Implement `turn` granularity: single `provider.complete` (no tool loop, no repo); fixture `system`/`expect`/`max_tokens` fields; coder turn auto-grades by `expect` substrings, reasoner turn graded manually; embedded `is-prime` turn fixture. Tests + live-verified against a local model.
- [x] 7.6 Save a reviewable transcript per trial: turn → full input context (system + prompt) + response (`transcript.txt`); phase/flow → preserve the run's `llm.jsonl` trace (`run-log.jsonl`) before deleting the run. `eval review` shows the transcript.
- [x] 7.7 Capture the assembled prompt in the agent loop: gated by `FLOWS_TRANSCRIPT` (set by eval), the agent writes a per-round transcript (system, tools, new input messages, response) to `<run_dir>/transcript.txt`; eval copies it into the trial dir. Live-verified on a phase run against a local model.
- [x] 7.8 `eval review` shows the transcript and collects a 5-dimension rating (correctness, efficiency, tool-use, grounding, instruction-following; 1-5 each, no skipping; tool-use n/a for turn). Pass derived from correctness ≥ 3. Ratings stored per trial; `report` shows per-dimension means (`C/E/T/G/I`). Live-verified.
- [x] 7.9 Full-screen review TUI (ratatui): scrollable transcript pane above a live ratings pane so the output stays visible while rating; keyboard + mouse-wheel scroll, 1-5 to rate, Tab/←→ move dims, n note, Enter save+next, q quit. Falls back to line prompts when not a terminal.
- [x] 7.10 Phase/flow fixtures use the REAL default prompts (not the bare fallback): add single-phase `implement` (→ `feature/implement.md`) and `plan` (→ read-only `feature/plan.md`) templates; retag coder-phase fixtures to `implement`, reasoner-phase to `plan`; flow fixtures already use `feature`. Turn stays eval-specific. Live-verified the real prompts render.

## 9. Reasoner alignment & context-preload

- [x] 9.1 Model tool-capability flag: `supports_tools: false` on a profile → runner sends no tool schemas; the model inlines file writes in `content.files`, which the runner writes to disk. Reasoner phases just emit their plan artifact (threaded via `prior_artifacts`, matching the original design).
- [ ] 9.2 (deferred) Context delivery: eval runs the REAL flow prompts unmodified — a read-incapable model's ungrounded output is the intended negative baseline. `relevant:` stays a label. A naive context feature will be added later and A/B'd against this baseline (no eval-side injection).

## 8. Store maintenance

- [x] 8.1 `eval clean` — delete stored results, filterable by `--model`/`--fixture`/`--suite`/`--granularity` (no filters = wipe the whole store); confirmation prompt unless `--yes`. Rewrites `results.jsonl` and removes the trial dirs. Store test for `remove_trials`.

## 6. Docs & wrap-up

- [x] 6.1 README `flows eval` section: run/list/review/report, fixture v2 format (suite/size/rubric/relevant), configs, the model axis.
- [x] 6.2 Full gate: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- [x] 6.3 `openspec validate eval-harness`.
