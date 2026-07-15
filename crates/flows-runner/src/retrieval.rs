//! Semantic retrieval (Tier 3): embed the repo's source in line-window chunks and
//! the phase idea, then inject the top-k most similar chunks into the prompt. This
//! is a **push** path — the model receives relevant code up front, so it works for
//! models that cannot call tools (many reasoning models).
//!
//! First cut: line-window chunking (no tree-sitter yet) and an in-memory index
//! (no on-disk cache / incremental re-embed yet) — both are follow-ups. The
//! embeddings endpoint is the run's configured LLM endpoint (`/v1/embeddings`).

use flows_llm::embeddings::embed;
use flows_llm::types::LlmError;
use ignore::WalkBuilder;
use std::path::Path;

use crate::repo_context::DENY;

/// Source extensions worth embedding. Keeps the index focused on code + docs and
/// skips binaries/data that would waste embedding calls.
const SOURCE_EXTS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "rb", "c", "h", "cpp", "hpp", "cc", "cs",
    "php", "swift", "kt", "scala", "sh", "toml", "yaml", "yml", "md", "sql",
];

/// Lines per chunk (~50 lines ≈ a few hundred tokens — a reasonable retrieval
/// granularity without symbol-aware splitting).
const CHUNK_LINES: usize = 50;
/// Hard cap on chunks embedded per run, so a huge repo can't explode the number
/// of embedding inputs. Chunks past this are dropped (retrieval stays best-effort).
const MAX_CHUNKS_EMBEDDED: usize = 600;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

/// Split every source file under `repo_root` into line-window chunks, respecting
/// `.gitignore` and the shared deny-list. Capped at [`MAX_CHUNKS_EMBEDDED`].
pub fn chunk_repo(repo_root: &Path) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let walk = WalkBuilder::new(repo_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();

    for entry in walk.flatten() {
        if chunks.len() >= MAX_CHUNKS_EMBEDDED {
            break;
        }
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let rel = path.strip_prefix(repo_root).unwrap_or(path);
        if rel
            .components()
            .any(|c| DENY.contains(&c.as_os_str().to_string_lossy().as_ref()))
        {
            continue;
        }
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| SOURCE_EXTS.contains(&e))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue; // skip unreadable / non-utf8
        };
        let rel_str = rel.to_string_lossy().into_owned();
        let lines: Vec<&str> = text.lines().collect();
        for (i, window) in lines.chunks(CHUNK_LINES).enumerate() {
            if window.iter().all(|l| l.trim().is_empty()) {
                continue; // skip blank windows
            }
            let start = i * CHUNK_LINES + 1;
            chunks.push(Chunk {
                path: rel_str.clone(),
                start_line: start,
                end_line: start + window.len() - 1,
                text: window.join("\n"),
            });
            if chunks.len() >= MAX_CHUNKS_EMBEDDED {
                break;
            }
        }
    }
    chunks
}

/// Cosine similarity. Returns 0 for a zero-length or mismatched vector.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Rank `chunks` against the query embedding and return the indices of the top
/// `k`, highest similarity first. Pure — unit-tested without a network call.
pub fn rank_top_k(query: &[f32], chunk_embs: &[Vec<f32>], k: usize) -> Vec<usize> {
    let mut scored: Vec<(usize, f32)> = chunk_embs
        .iter()
        .enumerate()
        .map(|(i, e)| (i, cosine(query, e)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.into_iter().take(k).map(|(i, _)| i).collect()
}

/// Format selected chunks as a labeled, injectable block.
fn format_chunks(chunks: &[Chunk], picks: &[usize]) -> String {
    let mut s = String::new();
    for &i in picks {
        let c = &chunks[i];
        s.push_str(&format!(
            "### {}:{}-{}\n```\n{}\n```\n\n",
            c.path, c.start_line, c.end_line, c.text
        ));
    }
    s.trim_end().to_string()
}

/// Embed the repo + the idea and return the top-`max_chunks` most relevant chunks
/// as an injectable string (empty when there's nothing to retrieve). Errors only
/// on embedding-endpoint failure.
pub async fn retrieve(
    repo_root: &Path,
    idea: &str,
    api_key: &str,
    api_base: &str,
    model: &str,
    max_chunks: usize,
) -> Result<String, LlmError> {
    let chunks = chunk_repo(repo_root);
    if chunks.is_empty() || max_chunks == 0 {
        return Ok(String::new());
    }
    // One batch: [idea, chunk_0, chunk_1, ...] so the query shares the request.
    let mut inputs = Vec::with_capacity(chunks.len() + 1);
    inputs.push(idea.to_string());
    inputs.extend(chunks.iter().map(|c| c.text.clone()));
    let embs = embed(api_key, api_base, model, &inputs).await?;
    let (query, chunk_embs) = embs
        .split_first()
        .ok_or_else(|| LlmError::Parse("embeddings response empty".into()))?;
    let picks = rank_top_k(query, chunk_embs, max_chunks);
    Ok(format_chunks(&chunks, &picks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cosine_basic() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0); // length mismatch
    }

    #[test]
    fn rank_picks_highest_similarity_first() {
        let q = vec![1.0, 0.0];
        let chunks = vec![
            vec![0.0, 1.0], // orthogonal
            vec![1.0, 0.0], // identical → best
            vec![0.9, 0.1], // close
        ];
        assert_eq!(rank_top_k(&q, &chunks, 2), vec![1, 2]);
    }

    #[test]
    fn chunks_source_files_only_and_windows() {
        let d = tempdir().unwrap();
        let r = d.path();
        // 120 lines → 3 windows of 50/50/20.
        let body: String = (1..=120)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(r.join("a.rs"), &body).unwrap();
        std::fs::write(r.join("data.bin"), "not source").unwrap();
        std::fs::create_dir_all(r.join("target")).unwrap();
        std::fs::write(r.join("target/skip.rs"), "ignored").unwrap();

        let chunks = chunk_repo(r);
        assert_eq!(chunks.len(), 3, "3 windows from a.rs, nothing else");
        assert!(chunks.iter().all(|c| c.path == "a.rs"));
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[1].start_line, 51);
        assert_eq!(chunks[2].start_line, 101);
        assert_eq!(chunks[2].end_line, 120);
    }
}
