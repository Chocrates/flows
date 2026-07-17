//! Semantic retrieval (Tier 3): embed the repo's source in line-window chunks and
//! the phase idea, then inject the top-k most similar chunks into the prompt. This
//! is a **push** path — the model receives relevant code up front, so it works for
//! models that cannot call tools (many reasoning models).
//!
//! Embeddings run **locally, in-process** via `fastembed` (ONNX) — no embeddings
//! server required. The model (a small sentence-transformer, default
//! all-MiniLM-L6-v2) is downloaded once to the fastembed cache and reused.
//!
//! The chunk index is built **once per flow** (memoized per process, keyed by repo
//! path + model) and **persisted** at `~/.flows/index/<repo-hash>/<model>.json`.
//! At flow start the persisted index is reused unless it is absent, the model or
//! `CHUNKER_VERSION` changed, or the fraction of changed files meets
//! `retrieval_reindex_threshold` — then it is rebuilt in full (D11). Only the
//! per-idea query vector is embedded on each retrieve call.
//!
//! Follow-ups: tree-sitter symbol chunking (currently line windows); a
//! content-addressed vector cache to avoid re-embedding on branch switches.

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::repo_context::DENY;

/// Bump when chunk boundaries change (invalidates every persisted index).
const CHUNKER_VERSION: u32 = 1;

/// Process-wide cache of loaded embedders, keyed by model name. Loading an ONNX
/// model is expensive (and downloads on first use), so we keep one per model for
/// the lifetime of the process. `embed` needs `&mut self`, hence the Mutex.
static EMBEDDERS: OnceLock<Mutex<HashMap<String, TextEmbedding>>> = OnceLock::new();

/// Total texts embedded this process — lets tests assert that a reused index does
/// not re-embed chunks (only the per-idea query).
#[cfg(test)]
static EMBED_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Map a config model name to a fastembed model. Unknown/empty → a small, fast
/// default (all-MiniLM-L6-v2, 384-dim). Kept small so first-run download + CPU
/// inference stay cheap.
fn resolve_model(name: &str) -> EmbeddingModel {
    match name.trim().to_ascii_lowercase().as_str() {
        "" | "minilm" | "all-minilm-l6-v2" | "allminilml6v2" => EmbeddingModel::AllMiniLML6V2,
        "bge-small" | "bge-small-en-v1.5" | "bgesmallenv15" => EmbeddingModel::BGESmallENV15,
        "bge-base" | "bge-base-en-v1.5" | "bgebaseenv15" => EmbeddingModel::BGEBaseENV15,
        _ => EmbeddingModel::AllMiniLML6V2,
    }
}

/// Embed a batch locally, loading (and caching) the model on first use. Blocking
/// CPU work — call from a blocking context (`spawn_blocking`).
fn embed_local(model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let mut map = EMBEDDERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "embedder cache poisoned".to_string())?;
    if !map.contains_key(model) {
        let emb = TextEmbedding::try_new(TextInitOptions::new(resolve_model(model)))
            .map_err(|e| format!("load embedding model `{model}`: {e}"))?;
        map.insert(model.to_string(), emb);
    }
    #[cfg(test)]
    EMBED_COUNT.fetch_add(texts.len(), std::sync::atomic::Ordering::Relaxed);
    let emb = map.get_mut(model).expect("just inserted");
    emb.embed(texts, None)
        .map_err(|e| format!("embed failed: {e}"))
}

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

/// Source files under `repo_root` worth indexing: `.gitignore`- and deny-list-
/// aware, filtered to [`SOURCE_EXTS`]. Returns `(repo-relative path, abs path)`,
/// sorted, so chunking and hashing see the exact same file set. Shared with the
/// repo map so all context features see one consistent file set.
pub(crate) fn source_files(repo_root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let walk = WalkBuilder::new(repo_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();
    for entry in walk.flatten() {
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
        if ext_ok {
            out.push((rel.to_string_lossy().into_owned(), path.to_path_buf()));
        }
    }
    out
}

/// blake3 content hash per source file, `path -> hex`. Drives the churn check.
fn source_file_hashes(repo_root: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (rel, abs) in source_files(repo_root) {
        if let Ok(bytes) = std::fs::read(&abs) {
            map.insert(rel, blake3::hash(&bytes).to_hex().to_string());
        }
    }
    map
}

/// Split every source file into line-window chunks. Capped at
/// [`MAX_CHUNKS_EMBEDDED`].
pub fn chunk_repo(repo_root: &Path) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    for (rel_str, abs) in source_files(repo_root) {
        if chunks.len() >= MAX_CHUNKS_EMBEDDED {
            break;
        }
        let Ok(text) = std::fs::read_to_string(&abs) else {
            continue; // skip unreadable / non-utf8
        };
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

/// A chunk plus its embedding — the persisted unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredChunk {
    path: String,
    start_line: usize,
    end_line: usize,
    text: String,
    vector: Vec<f32>,
}

/// The persisted repo index: chunk vectors plus the per-file hashes used to decide
/// whether to reuse or rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIndex {
    chunker_version: u32,
    model: String,
    files: BTreeMap<String, String>,
    chunks: Vec<StoredChunk>,
}

/// Process-wide memo of built indexes, keyed by `canonical-repo-path|model`, so a
/// flow embeds the repo at most once (every phase reuses it).
static INDEXES: OnceLock<Mutex<HashMap<String, Arc<RepoIndex>>>> = OnceLock::new();

fn flows_index_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".flows").join("index")
}

/// On-disk location: `~/.flows/index/<repo-hash>/<model>.json`, repo-hash from the
/// canonical path so different checkouts of the same path share, and moves don't.
fn index_path(repo_root: &Path, model: &str) -> PathBuf {
    let canon = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let repo_hash = blake3::hash(canon.to_string_lossy().as_bytes()).to_hex();
    let safe_model = if model.trim().is_empty() {
        "default"
    } else {
        model
    };
    flows_index_root()
        .join(&repo_hash.as_str()[..16])
        .join(format!("{safe_model}.json"))
}

/// Fraction of files that differ between two hash maps:
/// `(changed + added + removed) / max(|old|, |new|)`. 0 when both are empty.
fn churn(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> f64 {
    let added = new.keys().filter(|k| !old.contains_key(*k)).count();
    let removed = old.keys().filter(|k| !new.contains_key(*k)).count();
    let changed = new
        .iter()
        .filter(|(k, v)| old.get(*k).is_some_and(|ov| ov != *v))
        .count();
    let denom = old.len().max(new.len());
    if denom == 0 {
        0.0
    } else {
        (added + removed + changed) as f64 / denom as f64
    }
}

impl RepoIndex {
    /// Embed the whole repo and assemble a fresh index. Blocking.
    fn build(repo_root: &Path, model: &str) -> Result<RepoIndex, String> {
        let chunks = chunk_repo(repo_root);
        let files = source_file_hashes(repo_root);
        let vectors = if chunks.is_empty() {
            Vec::new()
        } else {
            embed_local(
                model,
                &chunks.iter().map(|c| c.text.clone()).collect::<Vec<_>>(),
            )?
        };
        let stored = chunks
            .into_iter()
            .zip(vectors)
            .map(|(c, vector)| StoredChunk {
                path: c.path,
                start_line: c.start_line,
                end_line: c.end_line,
                text: c.text,
                vector,
            })
            .collect();
        Ok(RepoIndex {
            chunker_version: CHUNKER_VERSION,
            model: model.to_string(),
            files,
            chunks: stored,
        })
    }

    fn load(path: &Path) -> Option<RepoIndex> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Atomic save: write a sibling temp file then rename.
    fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("index dir: {e}"))?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(self).map_err(|e| format!("serialize index: {e}"))?;
        std::fs::write(&tmp, bytes).map_err(|e| format!("write index: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("rename index: {e}"))
    }
}

/// Load-or-build the repo index, memoized per process (once per flow) and persisted
/// to disk. Rebuilds when there is no usable persisted index or churn ≥ threshold.
fn load_or_build_index(
    repo_root: &Path,
    model: &str,
    threshold: f64,
) -> Result<Arc<RepoIndex>, String> {
    let canon = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let key = format!("{}|{model}", canon.to_string_lossy());

    let mut memo = INDEXES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "index memo poisoned".to_string())?;
    if let Some(existing) = memo.get(&key) {
        return Ok(existing.clone());
    }

    let path = index_path(repo_root, model);
    let reuse = RepoIndex::load(&path).filter(|idx| {
        idx.chunker_version == CHUNKER_VERSION
            && idx.model == model
            && churn(&idx.files, &source_file_hashes(repo_root)) < threshold
    });
    let index = match reuse {
        Some(idx) => idx,
        None => {
            let idx = RepoIndex::build(repo_root, model)?;
            if let Err(e) = idx.save(&path) {
                eprintln!("  retrieval index save skipped: {e}");
            }
            idx
        }
    };
    let index = Arc::new(index);
    memo.insert(key, index.clone());
    Ok(index)
}

/// Retrieve the top-`max_chunks` chunks most relevant to `idea` as an injectable
/// string (empty when nothing to retrieve). Uses the per-flow persisted index for
/// chunk vectors; only the idea is embedded here. Blocking — run off the runtime.
pub fn retrieve(
    repo_root: &Path,
    idea: &str,
    model: &str,
    max_chunks: usize,
    reindex_threshold: f64,
) -> Result<String, String> {
    if max_chunks == 0 {
        return Ok(String::new());
    }
    let index = load_or_build_index(repo_root, model, reindex_threshold)?;
    if index.chunks.is_empty() {
        return Ok(String::new());
    }
    let query = embed_local(model, &[idea.to_string()])?
        .pop()
        .ok_or_else(|| "empty query embedding".to_string())?;
    let chunk_embs: Vec<Vec<f32>> = index.chunks.iter().map(|c| c.vector.clone()).collect();
    let picks = rank_top_k(&query, &chunk_embs, max_chunks);
    let chunks: Vec<Chunk> = index
        .chunks
        .iter()
        .map(|c| Chunk {
            path: c.path.clone(),
            start_line: c.start_line,
            end_line: c.end_line,
            text: c.text.clone(),
        })
        .collect();
    Ok(format_chunks(&chunks, &picks))
}

/// Ranked, de-duplicated file paths (most relevant first) by best-scoring chunk —
/// for the deterministic retrieval eval. Blocking (embeds the idea; builds/loads
/// the index).
pub fn rank_paths(
    repo_root: &Path,
    idea: &str,
    model: &str,
    reindex_threshold: f64,
) -> Result<Vec<String>, String> {
    let index = load_or_build_index(repo_root, model, reindex_threshold)?;
    if index.chunks.is_empty() {
        return Ok(Vec::new());
    }
    let query = embed_local(model, &[idea.to_string()])?
        .pop()
        .ok_or_else(|| "empty query embedding".to_string())?;
    let mut scored: Vec<(usize, f32)> = index
        .chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (i, cosine(&query, &c.vector)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    for (i, _) in scored {
        let p = &index.chunks[i].path;
        if seen.insert(p.clone()) {
            paths.push(p.clone());
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// End-to-end local embedding: downloads the default model on first run and
    /// ranks a query against real chunks. Ignored by default (network + ~90MB
    /// download + CPU); run with `cargo test -- --ignored retrieve_local`.
    #[test]
    #[ignore]
    fn retrieve_local_ranks_relevant_chunk_first() {
        let d = tempdir().unwrap();
        let r = d.path();
        std::fs::write(
            r.join("auth.rs"),
            "fn validate_token(t: &str) -> bool { verify signature and expiry }",
        )
        .unwrap();
        std::fs::write(r.join("math.rs"), "fn add(a: i32, b: i32) -> i32 { a + b }").unwrap();
        let out = retrieve(r, "how are auth tokens validated?", "", 1, 0.4).unwrap();
        assert!(out.contains("auth.rs"), "expected auth chunk, got: {out}");
        assert!(!out.contains("math.rs"));
    }

    /// Persisted index: first flow builds (embeds chunks), a later flow with an
    /// unchanged repo reuses it (embeds only the query), and a high-churn edit
    /// forces a rebuild. Ignored (downloads the model on first run).
    #[test]
    #[ignore]
    fn persisted_index_reused_until_churn() {
        use std::sync::atomic::Ordering;
        let reset = || {
            if let Some(m) = INDEXES.get() {
                m.lock().unwrap().clear();
            }
            EMBED_COUNT.store(0, Ordering::Relaxed);
        };
        let d = tempdir().unwrap();
        let r = d.path();
        std::fs::write(r.join("a.rs"), "fn login() { check password }").unwrap();
        std::fs::write(r.join("b.rs"), "fn parse() { read tokens }").unwrap();

        // Build.
        reset();
        retrieve(r, "how does login work?", "", 1, 0.4).unwrap();
        let built = EMBED_COUNT.load(Ordering::Relaxed);
        assert!(built >= 3, "build embeds 2 chunks + query, got {built}");

        // Reuse from disk (memo cleared, repo unchanged) → only the query embeds.
        reset();
        retrieve(r, "how does login work?", "", 1, 0.4).unwrap();
        assert_eq!(
            EMBED_COUNT.load(Ordering::Relaxed),
            1,
            "reuse should embed only the query"
        );

        // High churn (1 of 2 files changed = 0.5 ≥ 0.4) → rebuild.
        std::fs::write(r.join("a.rs"), "fn login() { verify OTP and password now }").unwrap();
        reset();
        retrieve(r, "how does login work?", "", 1, 0.4).unwrap();
        assert!(
            EMBED_COUNT.load(Ordering::Relaxed) >= 3,
            "high churn should rebuild (re-embed chunks)"
        );
    }

    #[test]
    fn churn_counts_add_remove_change() {
        let mut old = BTreeMap::new();
        old.insert("a".into(), "h1".to_string());
        old.insert("b".into(), "h2".to_string());
        // unchanged repo → 0.
        assert_eq!(churn(&old, &old), 0.0);
        // change b, add c → (1 changed + 1 added) / max(2,3) = 2/3.
        let mut new = BTreeMap::new();
        new.insert("a".into(), "h1".to_string());
        new.insert("b".into(), "h2-mod".to_string());
        new.insert("c".into(), "h3".to_string());
        assert!((churn(&old, &new) - 2.0 / 3.0).abs() < 1e-9);
        // both empty → 0 (no divide-by-zero).
        assert_eq!(churn(&BTreeMap::new(), &BTreeMap::new()), 0.0);
    }

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
