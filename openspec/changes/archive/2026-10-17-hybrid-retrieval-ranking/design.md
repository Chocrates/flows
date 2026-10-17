## Decisions

- **RRF over score blending.** Cosine and BM25 scores live on different scales; reciprocal-rank fusion (`1/(60+rank)` summed) needs no normalization and is robust. k = 60 is the standard constant.
- **BM25 over path + text**, tokens from the repo map tokenizer (camelCase/snake_case split, ≥ 3 chars) with light suffix stemming (`ing/ed/es/s`), k1 = 1.2, b = 0.75. Computed per query over the in-memory index (≤ 1500 chunks), no persisted lexical index.
- **Equal weights**: a sweep of lexical weight 0.5 / 0.75 / 1 / 1.5 gave MRR 0.67 / 0.69 / 0.70 / 0.71 and recall@5 0.73 / 0.71 / 0.74 / 0.71; 1.0 is the best recall with near-best MRR.
- **No flag**: retrieval itself is flag-gated; this changes its ranking based on the eval, like the symbol-chunking change did.
