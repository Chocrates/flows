## Context

`collect_ranked` already reads every supported file to extract signatures; `retrieval::bm25_scores` already implements stemmed BM25 for chunk-level lexical ranking.

## Goals / Non-Goals

**Goals:** better map ranking with no new dependency, model or config flag (the map is already behind `context.repo_map`).

**Non-Goals:** changing what the map renders; fusing the map with embeddings.

## Decisions

- **Document = path + full source.** Variants measured on the path-free corpus (MRR): signatures only 0.35, path + source 0.72, path + signatures + source 0.71, path doubled + source 0.72. The simplest wins.
- **Recency unchanged** (max 0.5 added to the BM25 score). The corpus has no git history, so it cannot tell weights apart; the existing tie-break tests still hold.
- **Sources are dropped after scoring** (`mem::take`) so the map does not hold the repo in memory while rendering.
- **BM25 builds one term-frequency map per document** for the query's terms instead of rescanning each document per term.

## Risks / Trade-offs

- [Huge generated files dominate by term frequency] → BM25 length normalisation damps them; `context.exclude` and `.gitignore` already drop most.
