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
//! At flow start it is synced with the repo: vectors are content-addressed by the
//! embedded text, so only new or changed chunks are embedded, and vectors of
//! chunks that disappear wait in a bounded spare pool (cheap branch switches).
//! A model or `CHUNKER_VERSION` change discards them. The index directory is
//! kept under a size cap (orphans first, then LRU). Only the per-idea query
//! vector is embedded on each retrieve call.

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::repo_context::DENY;

/// Bump when chunk boundaries change (invalidates every persisted index).
const CHUNKER_VERSION: u32 = 2;

/// Process-wide cache of loaded embedders, keyed by model name. Loading an ONNX
/// model is expensive (and downloads on first use), so we keep one per model for
/// the lifetime of the process. `embed` needs `&mut self`, hence the Mutex.
static EMBEDDERS: OnceLock<Mutex<HashMap<String, TextEmbedding>>> = OnceLock::new();

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

/// blake3 content hash per source file, `path -> hex`. An unchanged map lets a
/// persisted index skip re-chunking.
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

/// Light suffix stripping so "rounding"/"rounded"/"rounds" meet "round".
fn stem(t: &str) -> String {
    for suf in ["ing", "ed", "es", "s"] {
        if let Some(base) = t.strip_suffix(suf) {
            if base.len() >= 3 {
                return base.to_string();
            }
        }
    }
    t.to_string()
}

fn lexical_terms(s: &str) -> Vec<String> {
    crate::repo_map::token_list(s)
        .iter()
        .map(|t| stem(t))
        .collect()
}

/// Okapi BM25 score of each document for `query` (k1 = 1.2, b = 0.75).
pub(crate) fn bm25_scores(query: &str, docs: &[&str]) -> Vec<f64> {
    let (k1, b) = (1.2, 0.75);
    let docs: Vec<Vec<String>> = docs.iter().map(|d| lexical_terms(d)).collect();
    let n = docs.len() as f64;
    let avg = docs.iter().map(Vec::len).sum::<usize>() as f64 / n.max(1.0);
    let mut q: Vec<String> = lexical_terms(query);
    q.sort();
    q.dedup();
    let df: HashMap<&str, f64> = q
        .iter()
        .map(|t| {
            let c = docs.iter().filter(|d| d.iter().any(|x| x == t)).count();
            (t.as_str(), c as f64)
        })
        .collect();
    docs.iter()
        .map(|d| {
            let len = d.len() as f64;
            q.iter()
                .map(|t| {
                    let tf = d.iter().filter(|x| *x == t).count() as f64;
                    if tf == 0.0 {
                        return 0.0;
                    }
                    let idf = ((n - df[t.as_str()] + 0.5) / (df[t.as_str()] + 0.5) + 1.0).ln();
                    idf * tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * len / avg.max(1.0)))
                })
                .sum()
        })
        .collect()
}

/// Reciprocal-rank fusion of dense (cosine) and lexical (BM25) chunk rankings:
/// `Σ 1 / (60 + rank)`, equal weights. Returns chunk indices, best first.
/// Lexical matching recovers exact terms the small embedding model blurs; on
/// the path-free eval corpus fusion lifts MRR 0.65 → 0.70 (recall@5 0.70 → 0.74).
fn fused_order(query_text: &str, query_vec: &[f32], chunks: &[StoredChunk]) -> Vec<usize> {
    const K: f64 = 60.0;
    let rank_of = |scores: Vec<(usize, f64)>| {
        let mut s = scores;
        s.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut r = vec![0usize; s.len()];
        for (rank, (i, _)) in s.into_iter().enumerate() {
            r[i] = rank;
        }
        r
    };
    let dense = rank_of(
        chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (i, cosine(query_vec, &c.vector) as f64))
            .collect(),
    );
    let texts: Vec<String> = chunks
        .iter()
        .map(|c| format!("{}\n{}", c.path, c.text))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let lexical = rank_of(
        bm25_scores(query_text, &refs)
            .into_iter()
            .enumerate()
            .collect(),
    );
    let mut fused: Vec<(usize, f64)> = (0..chunks.len())
        .map(|i| {
            (
                i,
                1.0 / (K + dense[i] as f64) + 1.0 / (K + lexical[i] as f64),
            )
        })
        .collect();
    fused.sort_by(|a, b| b.1.total_cmp(&a.1));
    fused.into_iter().map(|(i, _)| i).collect()
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

/// A chunk plus its embedding — the persisted unit. `key` is the blake3 of the
/// exact text embedded, so a vector is reused only for identical input.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredChunk {
    path: String,
    start_line: usize,
    end_line: usize,
    text: String,
    #[serde(default)]
    key: String,
    vector: Vec<f32>,
}

/// A vector kept after its chunk left the repo (e.g. on a branch switch), so the
/// same text coming back needs no re-embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpareVector {
    key: String,
    vector: Vec<f32>,
}

/// Vectors kept in the spare pool (most recently dropped first).
const MAX_SPARE_VECTORS: usize = MAX_CHUNKS_EMBEDDED;

/// Size cap for `~/.flows/index/`, enforced after each save.
const INDEX_CACHE_MAX_BYTES: u64 = 1 << 30;

/// Marker in each repo-hash dir holding the canonical repo path, so pruning can
/// spot deleted checkouts without parsing (large) index files.
const REPO_MARKER: &str = "repo";

/// The persisted repo index: chunk vectors, the per-file hashes that let an
/// unchanged repo skip re-chunking, and the spare vector pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIndex {
    chunker_version: u32,
    model: String,
    files: BTreeMap<String, String>,
    chunks: Vec<StoredChunk>,
    #[serde(default)]
    spare: Vec<SpareVector>,
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

/// On-disk location: `<index_root>/<repo-hash>/<model>.json`, repo-hash from the
/// canonical path so different checkouts of the same path share, and moves don't.
fn index_path_in(index_root: &Path, canon_repo: &Path, model: &str) -> PathBuf {
    let repo_hash = blake3::hash(canon_repo.to_string_lossy().as_bytes()).to_hex();
    let safe_model = if model.trim().is_empty() {
        "default"
    } else {
        model
    };
    index_root
        .join(&repo_hash.as_str()[..16])
        .join(format!("{safe_model}.json"))
}

fn chunk_key(embed_text: &str) -> String {
    blake3::hash(embed_text.as_bytes()).to_hex().to_string()
}

/// Cap for each model's shared vector cache; past it, the newest half is kept.
const VECTOR_CACHE_MAX_BYTES: u64 = 256 << 20;

/// `~/.flows/vectors/<model>.bin`: chunk vectors by content key, shared by every
/// checkout, so the same code at another path (eval temp dirs, worktrees,
/// clones) is never embedded twice.
fn vector_cache_path(model: &str) -> PathBuf {
    let name: String = if model.trim().is_empty() {
        "default".into()
    } else {
        model
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    flows_index_root()
        .with_file_name("vectors")
        .join(format!("{name}.bin"))
}

/// One cache record: `[32-byte key][u32 LE dim][dim × f32 LE]`.
fn encode_vector(key: &str, v: &[f32], out: &mut Vec<u8>) -> bool {
    let Ok(hash) = blake3::Hash::from_hex(key) else {
        return false;
    };
    out.extend_from_slice(hash.as_bytes());
    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    true
}

/// Decode cache records in file order (later = newer). Stops at the first
/// incomplete record, so a truncated tail costs only that record.
fn decode_vectors(bytes: &[u8]) -> Vec<(String, Vec<f32>, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 36 <= bytes.len() {
        let key = blake3::Hash::from_bytes(bytes[i..i + 32].try_into().unwrap());
        let dim = u32::from_le_bytes(bytes[i + 32..i + 36].try_into().unwrap()) as usize;
        let end = i + 36 + dim * 4;
        if dim == 0 || end > bytes.len() {
            break;
        }
        let v = bytes[i + 36..end]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        out.push((key.to_hex().to_string(), v, i..end));
        i = end;
    }
    out
}

/// Keep only the newest records, up to `keep_bytes`. Temp file + rename.
fn compact_vector_cache(path: &Path, keep_bytes: u64) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    let records = decode_vectors(&bytes);
    let mut kept = 0u64;
    let mut start = records.len();
    for (idx, (_, _, r)) in records.iter().enumerate().rev() {
        if kept + r.len() as u64 > keep_bytes {
            break;
        }
        kept += r.len() as u64;
        start = idx;
    }
    let from = records
        .get(start)
        .map(|(_, _, r)| r.start)
        .unwrap_or(bytes.len());
    let to = records.last().map(|(_, _, r)| r.end).unwrap_or(from);
    let tmp = path.with_extension("bin.tmp");
    std::fs::write(&tmp, &bytes[from..to])?;
    std::fs::rename(&tmp, path)
}

/// Embed `texts` through the shared cache at `path`: cached keys are reused,
/// the rest go to `embed` (one call) and are appended. Cache I/O errors are
/// ignored — the cache only ever saves work.
fn cached_embed<E>(
    path: &Path,
    texts: &[String],
    embed: E,
    max_bytes: u64,
) -> Result<Vec<Vec<f32>>, String>
where
    E: FnOnce(&[String]) -> Result<Vec<Vec<f32>>, String>,
{
    let keys: Vec<String> = texts.iter().map(|t| chunk_key(t)).collect();
    let mut known: HashMap<String, Vec<f32>> = std::fs::read(path)
        .map(|b| {
            decode_vectors(&b)
                .into_iter()
                .map(|(k, v, _)| (k, v))
                .collect()
        })
        .unwrap_or_default();
    let missing: Vec<usize> = (0..texts.len())
        .filter(|&i| !known.contains_key(&keys[i]))
        .collect();
    if !missing.is_empty() {
        let batch: Vec<String> = missing.iter().map(|&i| texts[i].clone()).collect();
        let vectors = embed(&batch)?;
        if vectors.len() != batch.len() {
            return Err(format!(
                "embedder returned {} vectors for {} texts",
                vectors.len(),
                batch.len()
            ));
        }
        let mut buf = Vec::new();
        for (&i, v) in missing.iter().zip(vectors) {
            encode_vector(&keys[i], &v, &mut buf);
            known.insert(keys[i].clone(), v);
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        use std::io::Write;
        let appended = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(&buf));
        if appended.is_ok() && std::fs::metadata(path).is_ok_and(|m| m.len() > max_bytes) {
            let _ = compact_vector_cache(path, max_bytes / 2);
        }
    }
    Ok(keys.iter().map(|k| known[k].clone()).collect())
}

/// Bring `old` up to date with the repo: keep vectors whose key still appears,
/// embed only new/changed chunks (one `embed` call), and move vectors of chunks
/// that disappeared to the spare pool. `old` from another model or chunker
/// version is ignored. Unchanged file hashes return `old` untouched.
fn sync_index<E>(
    repo_root: &Path,
    model: &str,
    old: Option<RepoIndex>,
    warnings: &mut Vec<String>,
    embed: E,
) -> Result<RepoIndex, String>
where
    E: FnOnce(&[String]) -> Result<Vec<Vec<f32>>, String>,
{
    let old = old.filter(|o| o.chunker_version == CHUNKER_VERSION && o.model == model);
    let files = source_file_hashes(repo_root);
    if let Some(o) = old.as_ref().filter(|o| o.files == files) {
        return Ok(o.clone());
    }
    let (chunks, dropped) = chunk_repo_capped(repo_root, MAX_CHUNKS_EMBEDDED);
    if !dropped.is_empty() {
        warnings.push(format!(
            "retrieval index capped at {MAX_CHUNKS_EMBEDDED} chunks; {} file(s) not \
             fully indexed (e.g. {})",
            dropped.len(),
            dropped[0]
        ));
    }

    // Every vector we already have: current chunks first, then the spare pool.
    // Legacy chunks (saved before keys existed) can't be matched and drop out.
    let mut pool: Vec<(String, Vec<f32>)> = Vec::new();
    if let Some(o) = old {
        pool.extend(o.chunks.into_iter().map(|c| (c.key, c.vector)));
        pool.extend(o.spare.into_iter().map(|s| (s.key, s.vector)));
    }
    pool.retain(|(k, _)| !k.is_empty());
    let mut known: HashMap<String, Vec<f32>> = HashMap::new();
    for (k, v) in &pool {
        known.entry(k.clone()).or_insert_with(|| v.clone());
    }

    let texts: Vec<String> = chunks.iter().map(Chunk::embed_text).collect();
    let keys: Vec<String> = texts.iter().map(|t| chunk_key(t)).collect();
    let mut missing: Vec<usize> = Vec::new();
    let mut queued = HashSet::new();
    for (i, k) in keys.iter().enumerate() {
        if !known.contains_key(k) && queued.insert(k.as_str()) {
            missing.push(i);
        }
    }
    if !missing.is_empty() {
        let batch: Vec<String> = missing.iter().map(|&i| texts[i].clone()).collect();
        let vectors = embed(&batch)?;
        if vectors.len() != batch.len() {
            return Err(format!(
                "embedder returned {} vectors for {} chunks",
                vectors.len(),
                batch.len()
            ));
        }
        for (&i, v) in missing.iter().zip(vectors) {
            known.insert(keys[i].clone(), v);
        }
    }

    let used: HashSet<&str> = keys.iter().map(String::as_str).collect();
    let mut spare = Vec::new();
    let mut seen = HashSet::new();
    for (key, vector) in pool {
        if spare.len() >= MAX_SPARE_VECTORS {
            break;
        }
        if !used.contains(key.as_str()) && seen.insert(key.clone()) {
            spare.push(SpareVector { key, vector });
        }
    }

    let stored = chunks
        .into_iter()
        .zip(keys)
        .map(|(c, key)| StoredChunk {
            vector: known[&key].clone(),
            path: c.path,
            start_line: c.start_line,
            end_line: c.end_line,
            text: c.text,
            key,
        })
        .collect();
    Ok(RepoIndex {
        chunker_version: CHUNKER_VERSION,
        model: model.to_string(),
        files,
        chunks: stored,
        spare,
    })
}

impl RepoIndex {
    /// Load and mark as recently used (mtime drives LRU eviction).
    fn load(path: &Path) -> Option<RepoIndex> {
        let bytes = std::fs::read(path).ok()?;
        let idx = serde_json::from_slice(&bytes).ok()?;
        if let Ok(f) = std::fs::File::options().write(true).open(path) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        Some(idx)
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

/// Keep the index cache bounded: delete repo dirs whose marked repo no longer
/// exists, then the least-recently-used index files until the total is within
/// `max_bytes`. `keep` (the current repo's dir) is never touched. Best effort.
fn prune_index_cache(index_root: &Path, keep: &Path, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(index_root) else {
        return;
    };
    let mut candidates: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut total = 0u64;
    for dir in entries.flatten().map(|e| e.path()) {
        if !dir.is_dir() {
            continue;
        }
        if dir != keep {
            if let Ok(repo) = std::fs::read_to_string(dir.join(REPO_MARKER)) {
                if !Path::new(repo.trim_end()).exists() {
                    let _ = std::fs::remove_dir_all(&dir);
                    continue;
                }
            }
        }
        let Ok(inner) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in inner.flatten() {
            let Ok(meta) = f.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            total += meta.len();
            let p = f.path();
            if dir != keep && p.extension().is_some_and(|e| e == "json") {
                let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                candidates.push((mtime, meta.len(), p));
            }
        }
    }
    candidates.sort_by_key(|(mtime, _, _)| *mtime);
    for (_, len, path) in candidates {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_err() {
            continue;
        }
        total = total.saturating_sub(len);
        let Some(dir) = path.parent() else { continue };
        let only_marker = std::fs::read_dir(dir)
            .map(|d| d.flatten().all(|e| e.file_name() == REPO_MARKER))
            .unwrap_or(false);
        if only_marker {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Load-or-sync the repo index, memoized per process (once per flow) and
/// persisted to disk. Only new/changed chunks are embedded (see [`sync_index`]).
/// Non-fatal problems (e.g. the index couldn't be persisted) are pushed onto
/// `warnings` for the caller to surface — this module never prints.
fn load_or_build_index(
    repo_root: &Path,
    model: &str,
    warnings: &mut Vec<String>,
) -> Result<Arc<RepoIndex>, String> {
    let cache = vector_cache_path(model);
    load_or_build_index_in(&flows_index_root(), repo_root, model, warnings, |t| {
        cached_embed(&cache, t, |m| embed_local(model, m), VECTOR_CACHE_MAX_BYTES)
    })
}

fn load_or_build_index_in<E>(
    index_root: &Path,
    repo_root: &Path,
    model: &str,
    warnings: &mut Vec<String>,
    embed: E,
) -> Result<Arc<RepoIndex>, String>
where
    E: FnOnce(&[String]) -> Result<Vec<Vec<f32>>, String>,
{
    let canon = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let key = format!("{}|{model}", canon.to_string_lossy());

    let mut memo = crate::lock(INDEXES.get_or_init(|| Mutex::new(HashMap::new())));
    if let Some(existing) = memo.get(&key) {
        return Ok(existing.clone());
    }

    let path = index_path_in(index_root, &canon, model);
    let old = RepoIndex::load(&path);
    let stamp = |i: &RepoIndex| (i.chunker_version, i.model.clone(), i.files.clone());
    let on_disk = old.as_ref().map(stamp);
    let index = sync_index(repo_root, model, old, warnings, embed)?;
    if on_disk != Some(stamp(&index)) {
        persist_index(&index, &path, warnings);
        if let Some(dir) = path.parent() {
            let _ = std::fs::write(
                dir.join(REPO_MARKER),
                format!("{}\n", canon.to_string_lossy()),
            );
            prune_index_cache(index_root, dir, INDEX_CACHE_MAX_BYTES);
        }
    }
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
    warnings: &mut Vec<String>,
) -> Result<String, String> {
    if max_chunks == 0 || budget_tokens == 0 {
        return Ok(String::new());
    }
    let index = load_or_build_index(repo_root, model, warnings)?;
    if index.chunks.is_empty() {
        return Ok(String::new());
    }
    let query = embed_local(model, &[idea.to_string()])?
        .pop()
        .ok_or_else(|| "empty query embedding".to_string())?;
    let ranked = fused_order(idea, &query, &index.chunks);
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
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    let index = load_or_build_index(repo_root, model, warnings)?;
    if index.chunks.is_empty() {
        return Ok(Vec::new());
    }
    let query = embed_local(model, &[idea.to_string()])?
        .pop()
        .ok_or_else(|| "empty query embedding".to_string())?;
    let order = fused_order(idea, &query, &index.chunks);
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    for i in order {
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
    use std::cell::RefCell;
    use tempfile::tempdir;

    fn empty_index() -> RepoIndex {
        RepoIndex {
            chunker_version: CHUNKER_VERSION,
            model: String::new(),
            files: BTreeMap::new(),
            chunks: Vec::new(),
            spare: Vec::new(),
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
            &mut Vec::new(),
        )
        .unwrap();
        assert!(out.contains("auth.rs"), "expected auth chunk, got: {out}");
        assert!(!out.contains("math.rs"));
    }

    /// Fake embedder: records every text it is asked to embed.
    fn fake_embed(
        log: &RefCell<Vec<String>>,
    ) -> impl FnOnce(&[String]) -> Result<Vec<Vec<f32>>, String> + '_ {
        move |texts: &[String]| {
            log.borrow_mut().extend(texts.iter().cloned());
            Ok(texts.iter().map(|t| vec![t.len() as f32, 1.0]).collect())
        }
    }

    /// One sync through the real load/persist path, with the memo cleared so each
    /// call behaves like a new flow. Returns the texts embedded.
    fn sync_once(index_root: &Path, repo: &Path, model: &str) -> (Arc<RepoIndex>, Vec<String>) {
        if let Some(m) = INDEXES.get() {
            crate::lock(m).clear();
        }
        let log = RefCell::new(Vec::new());
        let idx =
            load_or_build_index_in(index_root, repo, model, &mut Vec::new(), fake_embed(&log))
                .unwrap();
        (idx, log.into_inner())
    }

    fn fn_src(name: &str) -> String {
        format!("fn {name}() {{\n    let x = 1;\n}}\n")
    }

    #[test]
    fn sync_embeds_only_new_or_changed_chunks() {
        let home = tempdir().unwrap();
        let d = tempdir().unwrap();
        let r = d.path();
        std::fs::write(r.join("a.rs"), fn_src("login")).unwrap();
        std::fs::write(r.join("b.rs"), fn_src("parse")).unwrap();

        let (idx, embedded) = sync_once(home.path(), r, "m");
        assert_eq!(embedded.len(), 2);
        assert!(idx.chunks.iter().all(|c| !c.key.is_empty()));

        // New flow, unchanged repo: nothing embedded.
        let (_, embedded) = sync_once(home.path(), r, "m");
        assert!(embedded.is_empty(), "{embedded:?}");

        // Edit one file: only its chunk is embedded, and the index sees the edit.
        std::fs::write(r.join("a.rs"), fn_src("login_with_otp")).unwrap();
        let (idx, embedded) = sync_once(home.path(), r, "m");
        assert_eq!(embedded.len(), 1);
        assert!(embedded[0].contains("login_with_otp"));
        assert!(idx.chunks.iter().any(|c| c.text.contains("login_with_otp")));
        assert!(!idx.chunks.iter().any(|c| c.text.contains("fn login()")));

        // "Switch back": the old text returns from the spare pool, no embedding.
        std::fs::write(r.join("a.rs"), fn_src("login")).unwrap();
        let (idx, embedded) = sync_once(home.path(), r, "m");
        assert!(embedded.is_empty(), "{embedded:?}");
        assert!(idx.chunks.iter().any(|c| c.text.contains("fn login()")));
        assert!(idx.spare.len() == 1, "otp version now spare");

        // Different model: everything is embedded again.
        let (_, embedded) = sync_once(home.path(), r, "other");
        assert_eq!(embedded.len(), 2);
    }

    #[test]
    fn legacy_index_without_keys_is_reembedded_once() {
        let home = tempdir().unwrap();
        let d = tempdir().unwrap();
        let r = d.path();
        std::fs::write(r.join("a.rs"), fn_src("login")).unwrap();
        let canon = r.canonicalize().unwrap();
        let path = index_path_in(home.path(), &canon, "m");
        // Saved by an older flows: no `key` / `spare`, files hash stale.
        let legacy = serde_json::json!({
            "chunker_version": CHUNKER_VERSION,
            "model": "m",
            "files": {"a.rs": "stale"},
            "chunks": [{"path": "a.rs", "start_line": 1, "end_line": 3,
                        "text": "fn login() {", "vector": [1.0, 2.0]}],
        });
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, legacy.to_string()).unwrap();
        let (_, embedded) = sync_once(home.path(), r, "m");
        assert_eq!(embedded.len(), 1);
        let (_, embedded) = sync_once(home.path(), r, "m");
        assert!(embedded.is_empty());
    }

    #[test]
    fn chunker_version_change_is_persisted() {
        let home = tempdir().unwrap();
        let d = tempdir().unwrap();
        let r = d.path();
        std::fs::write(r.join("a.rs"), fn_src("login")).unwrap();
        sync_once(home.path(), r, "m");
        let path = index_path_in(home.path(), &r.canonicalize().unwrap(), "m");
        let mut idx = RepoIndex::load(&path).unwrap();
        idx.chunker_version = CHUNKER_VERSION - 1;
        idx.save(&path).unwrap();
        let (_, embedded) = sync_once(home.path(), r, "m");
        assert_eq!(embedded.len(), 1);
        assert_eq!(
            RepoIndex::load(&path).unwrap().chunker_version,
            CHUNKER_VERSION
        );
    }

    fn cache_entry(root: &Path, name: &str, repo: &Path, bytes: usize, age_secs: u64) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(REPO_MARKER), format!("{}\n", repo.display())).unwrap();
        let f = dir.join("default.json");
        std::fs::write(&f, vec![b'x'; bytes]).unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(t)
            .unwrap();
        dir
    }

    #[test]
    fn prune_removes_orphans_then_lru_but_keeps_current() {
        let root = tempdir().unwrap();
        let live = tempdir().unwrap();
        let gone = live.path().join("deleted-checkout");
        let orphan = cache_entry(root.path(), "orphan", &gone, 10, 0);
        let oldest = cache_entry(root.path(), "oldest", live.path(), 1000, 300);
        let middle = cache_entry(root.path(), "middle", live.path(), 1000, 200);
        let newest = cache_entry(root.path(), "newest", live.path(), 1000, 100);
        // The current repo is the oldest of all, but must survive.
        let current = cache_entry(root.path(), "current", live.path(), 1000, 400);

        // Cap fits roughly three entries (markers count too).
        prune_index_cache(root.path(), &current, 3200);
        assert!(!orphan.exists(), "orphan pruned");
        assert!(!oldest.exists(), "LRU evicted first");
        assert!(middle.exists() && newest.exists() && current.exists());

        // Under the cap: nothing else goes.
        prune_index_cache(root.path(), &current, 1 << 20);
        assert!(middle.exists() && newest.exists());
    }

    #[test]
    fn load_marks_index_recently_used() {
        let root = tempdir().unwrap();
        let repo = tempdir().unwrap();
        let dir = cache_entry(root.path(), "x", repo.path(), 0, 1000);
        let f = dir.join("default.json");
        empty_index().save(&f).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(1000))
            .unwrap();
        assert!(RepoIndex::load(&f).is_some());
        let age = std::fs::metadata(&f)
            .unwrap()
            .modified()
            .unwrap()
            .elapsed()
            .unwrap();
        assert!(age.as_secs() < 60);
    }

    fn texts(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn vector_cache_skips_embedding_known_texts() {
        let d = tempdir().unwrap();
        let path = d.path().join("vectors").join("m.bin");
        let log = RefCell::new(Vec::new());
        let v1 = cached_embed(&path, &texts(&["a", "b"]), fake_embed(&log), u64::MAX).unwrap();
        assert_eq!(log.borrow().len(), 2);
        log.borrow_mut().clear();
        let v2 = cached_embed(&path, &texts(&["b", "c", "a"]), fake_embed(&log), u64::MAX).unwrap();
        assert_eq!(*log.borrow(), ["c"], "only the new text is embedded");
        assert_eq!(v2[0], v1[1]);
        assert_eq!(v2[2], v1[0]);
    }

    #[test]
    fn second_checkout_embeds_nothing() {
        let home = tempdir().unwrap();
        let cache = home.path().join("vectors.bin");
        let (one, two) = (tempdir().unwrap(), tempdir().unwrap());
        for r in [one.path(), two.path()] {
            std::fs::write(r.join("a.rs"), fn_src("login")).unwrap();
            std::fs::write(r.join("b.rs"), fn_src("parse")).unwrap();
        }
        let mut embedded = Vec::new();
        for r in [one.path(), two.path()] {
            if let Some(m) = INDEXES.get() {
                crate::lock(m).clear();
            }
            let log = RefCell::new(Vec::new());
            load_or_build_index_in(home.path(), r, "m", &mut Vec::new(), |t| {
                cached_embed(&cache, t, fake_embed(&log), u64::MAX)
            })
            .unwrap();
            embedded.push(log.into_inner().len());
        }
        assert_eq!(embedded, [2, 0]);
    }

    #[test]
    fn truncated_cache_keeps_intact_records() {
        let d = tempdir().unwrap();
        let path = d.path().join("m.bin");
        let log = RefCell::new(Vec::new());
        cached_embed(&path, &texts(&["a", "b"]), fake_embed(&log), u64::MAX).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
        assert_eq!(decode_vectors(&std::fs::read(&path).unwrap()).len(), 1);
        log.borrow_mut().clear();
        cached_embed(&path, &texts(&["a", "b"]), fake_embed(&log), u64::MAX).unwrap();
        assert_eq!(*log.borrow(), ["b"], "only the damaged record is redone");
    }

    #[test]
    fn cache_compacts_to_newest_records() {
        let d = tempdir().unwrap();
        let path = d.path().join("m.bin");
        let log = RefCell::new(Vec::new());
        cached_embed(&path, &texts(&["a", "b", "c"]), fake_embed(&log), u64::MAX).unwrap();
        let record = std::fs::metadata(&path).unwrap().len() / 3;
        // Appending "d" exceeds a 3-record cap → keep the newest 1.5 → 1 record.
        cached_embed(&path, &texts(&["d"]), fake_embed(&log), record * 3).unwrap();
        let keys: Vec<String> = decode_vectors(&std::fs::read(&path).unwrap())
            .into_iter()
            .map(|(k, _, _)| k)
            .collect();
        assert_eq!(keys, [chunk_key("d")]);
    }

    #[test]
    fn bm25_prefers_rare_matching_terms_and_stems() {
        let docs = [
            "fn round_cents(amount) { rounding to minor units }",
            "fn send_email(to) { smtp }",
            "fn total(lines) { sum amount amount amount }",
        ];
        let s = bm25_scores("why is rounding off by a cent", &docs);
        assert!(s[0] > s[1] && s[0] > s[2], "{s:?}");
        assert_eq!(s[1], 0.0);
    }

    fn stored(path: &str, text: &str, vector: Vec<f32>) -> StoredChunk {
        StoredChunk {
            path: path.into(),
            start_line: 1,
            end_line: 1,
            text: text.into(),
            key: String::new(),
            vector,
        }
    }

    #[test]
    fn fusion_lifts_an_exact_term_match_the_embedding_missed() {
        // Embedding prefers a.rs; only b.rs contains the query's rare terms.
        let chunks = vec![
            stored("a.rs", "fn unrelated() {}", vec![1.0, 0.0]),
            stored("b.rs", "fn webhook_signature() { hmac }", vec![0.0, 1.0]),
            stored("c.rs", "fn other() {}", vec![0.9, 0.1]),
        ];
        let order = fused_order("verify the webhook hmac signature", &[1.0, 0.0], &chunks);
        assert_eq!(order[0], 0, "the dense winner keeps the top spot");
        assert!(
            order.iter().position(|&i| i == 1) < order.iter().position(|&i| i == 2),
            "lexical hit beats the dense-only runner-up: {order:?}"
        );
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
