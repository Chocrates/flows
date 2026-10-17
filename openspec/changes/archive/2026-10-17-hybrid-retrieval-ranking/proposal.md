## Why

On the path-free retrieval corpus (68 queries) embedding-only ranking scores MRR 0.65. The small default embedding model blurs exact terms a query shares with the code (domain words, rare identifiers' parts), which a lexical ranker catches.

## What Changes

- Chunks are ranked by reciprocal-rank fusion of the embedding (cosine) ranking and a BM25 ranking over each chunk's path + text (camel/snake-aware tokens, light suffix stemming). Equal weights, k = 60.
- Applies to both injected chunks (`retrieve`) and the file ranking the eval scores (`rank_paths`).
- Measured: corpus MRR 0.65 → 0.70, recall@5 0.70 → 0.74. On the older path-naming fixtures recall@3/5 rise 0.89 → 0.94 while MRR falls 1.00 → 0.89 (one fixture's target drops to rank 3). Across all 74 queries MRR rises 0.68 → 0.72. A weight sweep (0.5–1.5) found equal weights best on recall@5.

## Capabilities

### Modified Capabilities
- `semantic-retrieval`: chunk relevance is a fusion of embedding and lexical ranking.

## Impact

`flows-runner/src/retrieval.rs` (BM25, fusion), `repo_map::token_list`; docs. No config change; retrieval stays behind `context.retrieval`.
