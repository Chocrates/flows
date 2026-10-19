## 1. Implementation

- [x] 1.1 `deletion_guard` module: snapshot via temp index, deleted paths between trees, restore, `DeletionGuard` with phase-start tree and limits
- [x] 1.2 `bash::invokes(command, program)` over the existing segment splitter; split unspaced `;` `|` `&` operators
- [x] 1.3 Wire the guard around bash dispatch in `agent.rs`; the restore error counts as refused
- [x] 1.4 Prompts: when to delete, one at a time, `files_deleted` in implement/review artifacts

## 2. Tests

- [x] 2.1 Guard unit tests: mass delete restored with contents (tracked and untracked), scratch deletes free, single obsolete file allowed, per-phase cap, index untouched
- [x] 2.2 Agent test with the mock provider: `rm -rf` of the repo is restored and the result is an error
- [x] 2.3 Non-git `rm` refused

## 3. Docs

- [x] 3.1 DESIGN.md, docs/ROADMAP.md, README security note
