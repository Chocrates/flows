## Why

In two end-to-end runs on a 27B local model, the `review` phase took longer than implementing: 63 and 109 minutes. It wrote throwaway programs to probe edge cases and rediscovered the change by reading files. The longer run hit its 40-round cap after lint and tests had passed, and that failed the whole run, so correct code was never committed. Review also cannot run `rm`, so the scratch files it left behind would be committed by `git add -A`.

## What Changes

- When only 3 rounds of a phase's budget are left, tools are switched off and the model is asked for its artifact (its verdict, for review). It no longer runs into a bare "max rounds exceeded" failure. Phases with a budget under 6 rounds keep the hard cap.
- Prompts get a `changes` variable: the files not yet tracked, then the uncommitted diff against `HEAD`, capped at 20k characters, and empty outside git. The fallback prompt shows it, `feature/review.md` reviews from it, and `feature/implement.md` shows it when looping back.
- `feature/review.md` asks for a proportionate review: edge cases go into real tests, and scratch files are deleted.
- The `feature` template's review allowlist gains `rm*`.

## Capabilities

### New Capabilities

### Modified Capabilities
- `agent-loop-guards`: the round cap ends with a forced reply.
- `repo-context`: the uncommitted changes are available to prompts.

## Impact

- `flows-runner::agent` (round loop, prompt context), `flows-runner::repo_context` (`working_changes`), the feature prompts, and `feature.yml`.
