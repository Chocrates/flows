## Why

Retrieval offers three embedding models (minilm, bge-small, bge-base). The BGE ones are used without the query instruction they were trained with, and nothing better is selectable. A sweep of nine fastembed models on the path-free corpus (68 queries) found EmbeddingGemma-300M at MRR 0.83 and jina-embeddings-v2-base-code at 0.77, against 0.71 for the default. The sweep also showed fastembed's default batch of 256 peaking at 3.6 GB for the larger models, and a mistyped model name silently falls back to minilm.

## What Changes

- `retrieval_model` accepts `jina-code` and `gemma` in addition to `minilm` (default), `bge-small` and `bge-base`.
- Each model embeds queries and chunks with the prefixes it was trained with (BGE query instruction; EmbeddingGemma code-retrieval prompts). Prefixed models store their index and vector cache under a separate name, so vectors embedded without prefixes are never reused.
- Embedding runs in batches of 8 (same speed or faster, memory 3.6 → 1.2 GB for gemma; minilm 0.87 → 0.28 GB).
- An unrecognised `retrieval_model` produces a run warning naming the known models.
- The default stays `minilm` (90 MB, fastest).

## Capabilities

### New Capabilities

### Modified Capabilities
- `semantic-retrieval`: selectable models with their prefixes, bounded embedding batches, unknown-model warning.

## Impact

- `flows-runner` `retrieval.rs`; docs (`configuration.md`, `eval.md`), ROADMAP.
