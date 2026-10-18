## 1. Implementation

- [x] 1.1 `FINAL_REPLY_ROUNDS` forced reply at the end of the round budget
- [x] 1.2 `repo_context::working_changes` and the `changes` prompt variable
- [x] 1.3 Fallback, `feature/review.md`, `feature/implement.md` show `changes`; review asks for proportionate review and cleanup
- [x] 1.4 `rm*` in the feature review allowlist

## 2. Tests

- [x] 2.1 Productive phase reaching the cap is forced to reply and returns its artifact
- [x] 2.2 `working_changes`: no repo, clean tree, new files before diff, truncation
- [x] 2.3 Fallback and feature review prompts render `changes`

## 3. Docs

- [x] 3.1 DESIGN.md, docs/templates.md, ROADMAP
