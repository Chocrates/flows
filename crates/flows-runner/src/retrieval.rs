//! Semantic retrieval (Tier 3): embed the repo's source in symbol-aware chunks
//! (see `chunker`) and the phase idea, then inject the most similar chunks that fit
//! the phase's token budget into the prompt. This
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
//! Follow-up: a content-addressed vector cache to avoid re-embedding on branch
//! switches.

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::repo_context::DENY;

/// Bump when chunk boundaries change (invalidates every persisted index).
const CHUNKER_VERSION: u32 = 2;

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
    let mut map = crate::lock(EMBEDDERS.get_or_init(|| Mutex::new(HashMap::new())));
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

/// Hard cap on chunks embedded per index build, so a huge repo can't explode the
/// number of embedding inputs. Code is indexed before docs/config, so the cap
/// drops the least useful files first; dropped files are reported as a warning.
const MAX_CHUNKS_EMBEDDED: usize = 1500;

/// Docs/config extensions: indexed only after all code, so they are the first to
/// go when a repo exceeds [`MAX_CHUNKS_EMBEDDED`].
const DOC_EXTS: &[&str] = &["md", "toml", "yaml", "yml", "sql"];

fn is_doc(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| DOC_EXTS.contains(&e))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    /// Enclosing context (e.g. `impl Foo for Bar {`) for a piece of a larger
    /// definition. Embedded, not injected.
    pub header: Option<String>,
}

impl Chunk {
    /// What gets embedded: the path (so path words match the idea, as they do for
    /// the repo map), any enclosing header, then the source.
    fn embed_text(&self) -> String {
        match &self.header {
            Some(h) => format!("{}\n{h}\n{}", self.path, self.text),
            None => format!("{}\n{}", self.path, self.text),
        }
    }
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

/// Split every source file into symbol-aware chunks (see `chunker`). Capped at
/// [`MAX_CHUNKS_EMBEDDED`].
pub fn chunk_repo(repo_root: &Path) -> Vec<Chunk> {
    chunk_repo_capped(repo_root, MAX_CHUNKS_EMBEDDED).0
}

/// Chunk code files first, then docs/config, stopping at `cap` chunks. Returns
/// the chunks and the repo-relative paths of files left (wholly or partly)
/// unindexed.
fn chunk_repo_capped(repo_root: &Path, cap: usize) -> (Vec<Chunk>, Vec<String>) {
    let mut files = source_files(repo_root);
    files.sort_by_key(|(_, abs)| is_doc(abs)); // stable: code first, path order kept
    let mut chunks = Vec::new();
    let mut dropped = Vec::new();
    for (rel_str, abs) in files {
        if chunks.len() >= cap {
            dropped.push(rel_str);
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&abs) else {
            continue; // skip unreadable / non-utf8
        };
        let lines: Vec<&str> = text.lines().collect();
        let ext = abs.extension().and_then(|e| e.to_str());
        for piece in crate::chunker::chunk_source(ext, &text) {
            if chunks.len() >= cap {
                dropped.push(rel_str.clone());
                break;
            }
            chunks.push(Chunk {
                path: rel_str.clone(),
                start_line: piece.start + 1,
                end_line: piece.end + 1,
                text: lines[piece.start..=piece.end].join("\n"),
                header: piece.header,
            });
        }
    }
    (chunks, dropped)
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

/// One chunk as injected: a `### path:start-end` header plus a fenced block.
fn format_chunk(c: &Chunk) -> String {
    format!(
        "### {}:{}-{}\n```\n{}\n```\n\n",
        c.path, c.start_line, c.end_line, c.text
    )
}

/// Format selected chunks as a labeled, injectable block.
fn format_chunks(chunks: &[Chunk], picks: &[usize]) -> String {
    let s: String = picks.iter().map(|&i| format_chunk(&chunks[i])).collect();
    s.trim_end().to_string()
}

/// Best-first packing: walk `ranked` (most relevant first) and keep each chunk
/// whose formatted size fits the remaining `budget_tokens`, skipping ones that
/// don't (so one huge top hit can't starve the rest). Stops at `max_chunks`.
pub fn pack_chunks(
    chunks: &[Chunk],
    ranked: &[usize],
    max_chunks: usize,
    budget_tokens: usize,
) -> Vec<usize> {
    let mut picks = Vec::new();
    let mut left = budget_tokens;
    for &i in ranked {
        if picks.len() >= max_chunks || left == 0 {
            break;
        }
        let cost = crate::budget::est_str(&format_chunk(&chunks[i]));
        if cost <= left {
            picks.push(i);
            left -= cost;
        }
    }
    picks
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
    fn build(
        repo_root: &Path,
        model: &str,
        warnings: &mut Vec<String>,
    ) -> Result<RepoIndex, String> {
        let (chunks, dropped) = chunk_repo_capped(repo_root, MAX_CHUNKS_EMBEDDED);
        if !dropped.is_empty() {
            warnings.push(format!(
                "retrieval index capped at {MAX_CHUNKS_EMBEDDED} chunks; {} file(s) not \
                 fully indexed (e.g. {})",
                dropped.len(),
                dropped[0]
            ));
        }
        let files = source_file_hashes(repo_root);
        let vectors = if chunks.is_empty() {
            Vec::new()
        } else {
            embed_local(
                model,
                &chunks.iter().map(Chunk::embed_text).collect::<Vec<_>>(),
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

/// Best-effort save: a failure costs only cross-flow reuse, so it becomes a
/// warning rather than an error.
fn persist_index(idx: &RepoIndex, path: &Path, warnings: &mut Vec<String>) {
    if let Err(e) = idx.save(path) {
        warnings.push(format!("retrieval index save skipped: {e}"));
    }
}

/// Load-or-build the repo index, memoized per process (once per flow) and persisted
/// to disk. Rebuilds when there is no usable persisted index or churn ≥ threshold.
/// Non-fatal problems (e.g. the index couldn't be persisted) are pushed onto
/// `warnings` for the caller to surface — this module never prints.
fn load_or_build_index(
    repo_root: &Path,
    model: &str,
    threshold: f64,
    warnings: &mut Vec<String>,
) -> Result<Arc<RepoIndex>, String> {
    let canon = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let key = format!("{}|{model}", canon.to_string_lossy());

    let mut memo = crate::lock(INDEXES.get_or_init(|| Mutex::new(HashMap::new())));
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
            let idx = RepoIndex::build(repo_root, model, warnings)?;
            persist_index(&idx, &path, warnings);
            idx
        }
    };
    let index = Arc::new(index);
    memo.insert(key, index.clone());
    Ok(index)
}

/// Retrieve the chunks most relevant to `idea` as an injectable string (empty
/// when nothing to retrieve), packed best-first into `budget_tokens` and capped at
/// `max_chunks`. Uses the per-flow persisted index for chunk vectors; only the
/// idea is embedded here. Blocking — run off the runtime. Non-fatal problems are
/// appended to `warnings`.
pub fn retrieve(
    repo_root: &Path,
    idea: &str,
    model: &str,
    max_chunks: usize,
    budget_tokens: usize,
    reindex_threshold: f64,
    warnings: &mut Vec<String>,
) -> Result<String, String> {
    if max_chunks == 0 || budget_tokens == 0 {
        return Ok(String::new());
    }
    let index = load_or_build_index(repo_root, model, reindex_threshold, warnings)?;
    if index.chunks.is_empty() {
        return Ok(String::new());
    }
    let query = embed_local(model, &[idea.to_string()])?
        .pop()
        .ok_or_else(|| "empty query embedding".to_string())?;
    let chunk_embs: Vec<Vec<f32>> = index.chunks.iter().map(|c| c.vector.clone()).collect();
    let ranked = rank_top_k(&query, &chunk_embs, chunk_embs.len());
    let chunks: Vec<Chunk> = index
        .chunks
        .iter()
        .map(|c| Chunk {
            path: c.path.clone(),
            start_line: c.start_line,
            end_line: c.end_line,
            text: c.text.clone(),
            header: None,
        })
        .collect();
    let picks = pack_chunks(&chunks, &ranked, max_chunks, budget_tokens);
    Ok(format_chunks(&chunks, &picks))
}

/// Ranked, de-duplicated file paths (most relevant first) by best-scoring chunk —
/// for the deterministic retrieval eval. Blocking (embeds the idea; builds/loads
/// the index). Non-fatal problems are appended to `warnings`.
pub fn rank_paths(
    repo_root: &Path,
    idea: &str,
    model: &str,
    reindex_threshold: f64,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    let index = load_or_build_index(repo_root, model, reindex_threshold, warnings)?;
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

    fn empty_index() -> RepoIndex {
        RepoIndex {
            chunker_version: CHUNKER_VERSION,
            model: String::new(),
            files: BTreeMap::new(),
            chunks: Vec::new(),
        }
    }

    fn chunk_of(len: usize) -> Chunk {
        Chunk {
            path: "f.rs".into(),
            start_line: 1,
            end_line: 1,
            text: "x".repeat(len),
            header: None,
        }
    }

    fn cost(c: &Chunk) -> usize {
        crate::budget::est_str(&format_chunk(c))
    }

    #[test]
    fn pack_stops_at_chunk_cap() {
        let chunks: Vec<Chunk> = (0..5).map(|_| chunk_of(40)).collect();
        assert_eq!(
            pack_chunks(&chunks, &[4, 3, 2, 1, 0], 2, usize::MAX),
            vec![4, 3]
        );
    }

    #[test]
    fn pack_stays_within_token_budget() {
        let chunks: Vec<Chunk> = (0..5).map(|_| chunk_of(400)).collect();
        let budget = cost(&chunks[0]) * 2 + 1;
        let picks = pack_chunks(&chunks, &[0, 1, 2, 3, 4], 10, budget);
        assert_eq!(picks, vec![0, 1]);
        let used: usize = picks.iter().map(|&i| cost(&chunks[i])).sum();
        assert!(used <= budget);
    }

    #[test]
    fn pack_skips_oversized_top_hit() {
        // The best hit alone exceeds the budget; smaller lower-ranked chunks still fit.
        let chunks = vec![chunk_of(4000), chunk_of(40), chunk_of(40)];
        let budget = cost(&chunks[1]) * 2;
        assert_eq!(pack_chunks(&chunks, &[0, 1, 2], 10, budget), vec![1, 2]);
    }

    #[test]
    fn pack_with_zero_budget_is_empty() {
        let chunks = vec![chunk_of(10)];
        assert!(pack_chunks(&chunks, &[0], 10, 0).is_empty());
    }

    #[test]
    fn persist_failure_is_a_warning_not_an_error() {
        // Parent "directory" is a regular file, so create_dir_all fails.
        let d = tempdir().unwrap();
        let blocker = d.path().join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let mut warnings = Vec::new();
        persist_index(&empty_index(), &blocker.join("idx.json"), &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("index save skipped"), "{warnings:?}");
    }

    #[test]
    fn persist_success_leaves_no_warning() {
        let d = tempdir().unwrap();
        let path = d.path().join("sub").join("idx.json");
        let mut warnings = Vec::new();
        persist_index(&empty_index(), &path, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(RepoIndex::load(&path).is_some());
    }

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
        let out = retrieve(
            r,
            "how are auth tokens validated?",
            "",
            1,
            usize::MAX,
            0.4,
            &mut Vec::new(),
        )
        .unwrap();
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
                crate::lock(m).clear();
            }
            EMBED_COUNT.store(0, Ordering::Relaxed);
        };
        let d = tempdir().unwrap();
        let r = d.path();
        std::fs::write(r.join("a.rs"), "fn login() { check password }").unwrap();
        std::fs::write(r.join("b.rs"), "fn parse() { read tokens }").unwrap();

        // Build.
        reset();
        retrieve(
            r,
            "how does login work?",
            "",
            1,
            usize::MAX,
            0.4,
            &mut Vec::new(),
        )
        .unwrap();
        let built = EMBED_COUNT.load(Ordering::Relaxed);
        assert!(built >= 3, "build embeds 2 chunks + query, got {built}");

        // Reuse from disk (memo cleared, repo unchanged) → only the query embeds.
        reset();
        retrieve(
            r,
            "how does login work?",
            "",
            1,
            usize::MAX,
            0.4,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            EMBED_COUNT.load(Ordering::Relaxed),
            1,
            "reuse should embed only the query"
        );

        // High churn (1 of 2 files changed = 0.5 ≥ 0.4) → rebuild.
        std::fs::write(r.join("a.rs"), "fn login() { verify OTP and password now }").unwrap();
        reset();
        retrieve(
            r,
            "how does login work?",
            "",
            1,
            usize::MAX,
            0.4,
            &mut Vec::new(),
        )
        .unwrap();
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
        std::fs::write(r.join("a.md"), &body).unwrap();
        std::fs::write(r.join("data.bin"), "not source").unwrap();
        std::fs::create_dir_all(r.join("target")).unwrap();
        std::fs::write(r.join("target/skip.rs"), "ignored").unwrap();

        let chunks = chunk_repo(r);
        assert_eq!(chunks.len(), 3, "3 windows from a.md, nothing else");
        assert!(chunks.iter().all(|c| c.path == "a.md"));
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[1].start_line, 51);
        assert_eq!(chunks[2].start_line, 101);
        assert_eq!(chunks[2].end_line, 120);
    }

    #[test]
    fn cap_drops_docs_before_code_and_reports_them() {
        let d = tempdir().unwrap();
        let r = d.path();
        // `a_notes.md` sorts first by path but must be dropped before code.
        std::fs::write(r.join("a_notes.md"), "# notes\nsome docs\n").unwrap();
        std::fs::write(r.join("b.rs"), "fn b() {}\n").unwrap();
        std::fs::write(r.join("c.py"), "def c():\n    pass\n").unwrap();
        let (chunks, dropped) = chunk_repo_capped(r, 2);
        let paths: Vec<&str> = chunks.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["b.rs", "c.py"]);
        assert_eq!(dropped, vec!["a_notes.md".to_string()]);
        let (all, none) = chunk_repo_capped(r, 10);
        assert_eq!(all.len(), 3);
        assert!(none.is_empty());
    }

    #[test]
    fn rust_files_chunk_at_symbols_with_1_based_lines() {
        let d = tempdir().unwrap();
        let r = d.path();
        let big = |name: &str| {
            let body: String = (0..40)
                .map(|i| format!("    let v{i} = {i}; // padding padding padding\n"))
                .collect();
            format!("fn {name}() {{\n{body}}}\n")
        };
        std::fs::write(r.join("lib.rs"), format!("{}\n{}", big("one"), big("two"))).unwrap();
        let chunks = chunk_repo(r);
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        assert_eq!(chunks[0].start_line, 1);
        assert!(chunks[0].text.starts_with("fn one()"));
        assert!(chunks[1].text.trim_start().starts_with("fn two()"));
        assert_eq!(
            chunks[1].text.lines().count(),
            chunks[1].end_line - chunks[1].start_line + 1
        );
        assert!(chunks[0].embed_text().starts_with("lib.rs\nfn one()"));
    }
}
