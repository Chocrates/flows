## 1. Implementation

- [x] 1.1 `query_terms` / `rephrased`; rephrased `web_search` / `lookup` calls are not fresh
- [x] 1.2 `files_written` guard skips when `working_changes` is non-empty

## 2. Tests

- [x] 2.1 Real reworded queries flagged; unrelated ones not
- [x] 2.2 Verify-only pass over uncommitted changes returns its artifact; the old guard test still fails outside git
