## 1. Implementation

- [x] 1.1 Refuse `lookup` / `web_search` queries with < 3 alphanumerics
- [x] 1.2 Repo-relative hint on refused absolute paths that exist in the repo

## 2. Tests

- [x] 2.1 Agent test: alphabet-walk lookups are refused, nudged, then forced
- [x] 2.2 File tests: hint for "/src/a.rs", none for "/etc/passwd"
