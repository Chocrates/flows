## 1. Core

- [x] 1.1 Add `flows_core::condition` (compile check, `eval_when` with `artifacts` + `idea`, chainable undefined, tuple rewrite) with unit tests; runner's `eval_loop_condition` reuses the tuple rewrite
- [x] 1.2 Add `PhaseConfig.when`, reject unparsable conditions in `validate_pipeline`, add `RunState.skipped_phases`, show conditions in `build_plan_lines`; tests

## 2. Runner

- [x] 2.1 Add `RunnerCallbacks::on_phase_skipped`; evaluate `when` in the linear loop (skip artifact, completed + skipped lists, error fails the run); tests in `tests/runner.rs`
- [x] 2.2 Evaluate `when` in the DAG scheduler (skip counts as satisfied, rescheduling after skips); tests

## 3. UI + docs

- [x] 3.1 TUI `PhaseStatus::Skipped` (⊘) from the live event and from `skipped_phases` in attach; foreground CLI prints the skip; tests
- [x] 3.2 Document `when:` in the template docs and README; update ROADMAP/DESIGN
