## Why

The retrieval eval scores six fixtures, all from this repository, and most tasks name the file to change. Path-matching rankers look far better than they are: the repo map scores MRR 0.47 and embedding retrieval 1.00 on them, but a first path-free query set over the same code drops them to 0.11 and 0.46. Ranking changes can't be judged on that corpus.

## What Changes

- **Retrieval corpora**: a folder with `queries.yml` (many `{task, relevant}` pairs) and a `repo/` tree, or `repo:` pointing at another fixture's tree. One index per corpus serves all its queries.
- `flows eval retrieval --corpus DIR` scores corpora (alone, or alongside `--fixtures`); `--per-query` prints a row per query. Rows gain a query count `n`; the overall MEAN weights every query equally.
- New corpus under `eval-fixtures/retrieval/`: path-free queries (no file, directory, or identifier names) over the flows workspace snapshot plus synthetic Python, TypeScript, and Go projects.
- Labels are validated: a `relevant` path missing from the repo is an error.

## Capabilities

### Modified Capabilities
- `context-eval`: deterministic retrieval evaluation gains multi-query corpora.

## Impact

`flows-cli` (`eval.rs`, `main.rs`), `eval-fixtures/retrieval/`, docs (`docs/eval.md`, `eval-fixtures/README.md`), ROADMAP. No runtime behavior changes.
