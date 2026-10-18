## Why

Templates whose phases have no prompt file (the `e2e` scratch template, `blank`, most user templates) get the generic fallback prompt. It rendered only the task and the file tree, so:

- `implement` never saw the `plan` artifact, nor `review` the implementation summary: earlier phases' work was invisible.
- `context.repo_map` and retrieval were computed but dropped, so `--context` had no effect on such templates. An e2e comparison of context off vs on was identical up to the first prompt.
- The tree printed directories with a leading slash (`/internal`), and the model kept requesting `/internal/schedule/cron.go`, which is refused as an absolute path.

## What Changes

- The fallback prompt includes each earlier phase's artifact content (capped at 6000 characters each), the repo map, pushed context (retrieval/oracle), and the tool list, each only when present.
- The repo tree (full and slim) marks directories as `name/`.

## Capabilities

### New Capabilities

### Modified Capabilities
- `repo-context`: fallback prompts carry prior artifacts and pushed context; tree directories end with `/`.

## Impact

- `flows-runner` `prompts.rs`, `repo_context.rs`.
