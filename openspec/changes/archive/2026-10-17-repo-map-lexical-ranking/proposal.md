## Why

The repo map ranks files by how many idea words appear in their path and signatures, counted equally. On the path-free retrieval corpus (68 queries) it scores MRR 0.34 against 0.71 for embedding retrieval: ideas describe behaviour ("pending edits should go out when the connection returns"), which shows up in bodies and comments, not signatures, and common words ("the", "when", "should") count as much as rare ones.

## What Changes

- Rank map files by BM25 (stemmed, IDF-weighted) of the idea against each file's path and full source; the rendered map still shows only signatures.
- Recency stays a bounded boost that orders near-ties.
- BM25 scoring counts term frequencies once per document (shared with retrieval's lexical ranker), so ranking full sources stays cheap.

Measured with `flows eval retrieval`: path-free corpus map MRR 0.34 → 0.72 (r@5 0.40 → 0.73); labeled fixtures map MRR 0.47 → 1.00. On the Rust standard library (1.9k files, 52 MB) BM25 adds ~1.4 s to map building, which is dominated by tree-sitter parsing.

## Capabilities

### New Capabilities

### Modified Capabilities
- `repo-context`: map relevance is lexical (BM25) over path and full source.

## Impact

- `flows-runner` `repo_map.rs`, `retrieval.rs` (`bm25_scores`); DESIGN.md, ROADMAP.
