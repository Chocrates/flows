## 1. Implementation

- [x] 1.1 Rank map files by BM25 over path + full source, plus recency
- [x] 1.2 `bm25_scores` counts term frequencies once per document

## 2. Tests and docs

- [x] 2.1 Unit test: body-only rare term outranks files sharing common words; existing ranking/recency tests pass
- [x] 2.2 Measure with `flows eval retrieval` (corpus + fixtures) and on a large tree
- [x] 2.3 DESIGN.md repo-map notes, ROADMAP
