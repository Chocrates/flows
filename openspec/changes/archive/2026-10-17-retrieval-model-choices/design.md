## Context

`resolve_model` maps a config name to a fastembed `EmbeddingModel`; one `embed_local` serves chunks and queries. Indexes and the shared vector cache are keyed by the configured model name.

## Goals / Non-Goals

**Goals:** let users pick a stronger local model, measured; keep default runs cheap.

**Non-Goals:** changing the default (a 1.2 GB download and ~10× indexing time is the user's call); hosted embedding endpoints.

## Decisions

- **Sweep** (`flows eval retrieval --corpus eval-fixtures/retrieval --model <m>`, rag MRR): minilm 0.71, bge-small 0.72, bge-base 0.72, jina-code 0.77, gemma 0.83, gte-base 0.74, nomic 0.65, arctic-s 0.67, e5-small 0.67. Offered: the existing three plus jina-code and gemma. The rest are no better than a cheaper option.
- **Quantized EmbeddingGemma excluded.** fastembed's dynamic quantization refuses batching, so a whole index embeds in one batch (27 GB peak on the corpus).
- **Prefixes in a `ModelSpec`** (`query_prefix`, `doc_prefix`), applied in `embed_query` / `embed_docs`. Chunk keys stay content hashes; the prefix is part of the model identity instead, via the `@prefixed` storage name.
- **Batch 8 for every model.** Measured cold on ts-kanban: minilm 6.1 s / 0.87 GB at 256 vs 4.9 s / 0.28 GB at 8; gemma 54 s / 3.6 GB vs 50 s / 1.2 GB.
- **Unknown-name warning** once per index build, through the existing retrieval warnings (shown as run notes).

## Risks / Trade-offs

- [bge-small/bge-base users re-embed once after upgrading] → expected; their old vectors lacked the prefix.
