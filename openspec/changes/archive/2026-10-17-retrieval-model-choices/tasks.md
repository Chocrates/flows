## 1. Implementation

- [x] 1.1 `ModelSpec` with query/doc prefixes; `jina-code` and `gemma` names
- [x] 1.2 `embed_query` / `embed_docs`; `@prefixed` storage name for index + vector cache
- [x] 1.3 Fixed embedding batch of 8
- [x] 1.4 Unknown-model warning once per index build

## 2. Tests and docs

- [x] 2.1 Unit tests: resolution + prefixes, storage names, unknown-name warning
- [x] 2.2 Model sweep with `flows eval retrieval`, memory/time per model
- [x] 2.3 configuration.md, eval.md tables, ROADMAP
