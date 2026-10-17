//! Repo map (Tier 2): a symbol-level index of the codebase. For each source file
//! in a supported language, extract its top-level symbol **signatures** (function /
//! type / class / trait / … headers) via tree-sitter, rank files by relevance to
//! the run's idea, and render a token-bounded map. This is **push** context — a
//! model sees the shape of the codebase (what's defined where) without whole files
//! or tool calls, so it helps small and no-tool models alike.
//!
//! Signatures are the first line of each definition node (up to the body), which is
//! cheap and language-agnostic enough for a map. Ranking is BM25 of the idea
//! against each file's path + full source (identifiers, comments, strings), plus
//! a small recency boost: from git history when the repo has one (see
//! [`git_recency`]), else from file mtimes.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Node, Parser};

use crate::retrieval::source_files;

/// ~4 chars/token heuristic, matching the budgeting estimator.
fn est_tokens(s: &str) -> usize {
    s.len() / 4 + 1
}

#[derive(Clone, Copy)]
pub(crate) enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
}

impl Lang {
    pub(crate) fn from_ext(ext: &str) -> Option<Lang> {
        Some(match ext {
            "rs" => Lang::Rust,
            "py" => Lang::Python,
            "js" | "jsx" => Lang::JavaScript,
            "ts" | "tsx" => Lang::TypeScript,
            "go" => Lang::Go,
            _ => return None,
        })
    }

    pub(crate) fn ts_language(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }

    /// Node kinds that name a definition worth listing in the map.
    fn def_kinds(self) -> &'static [&'static str] {
        match self {
            Lang::Rust => &[
                "function_item",
                "struct_item",
                "enum_item",
                "trait_item",
                "type_item",
                "mod_item",
                "macro_definition",
                "const_item",
                "static_item",
            ],
            Lang::Python => &["function_definition", "class_definition"],
            Lang::JavaScript => &[
                "function_declaration",
                "class_declaration",
                "method_definition",
            ],
            Lang::TypeScript => &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
                "enum_declaration",
            ],
            Lang::Go => &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
            ],
        }
    }
}

/// One file's extracted signatures.
struct FileSymbols {
    path: String,
    signatures: Vec<String>,
    mtime: u64,
    /// Full source, used for ranking only (taken once scored).
    src: String,
}

/// The first line of a node's source, trimmed and with a trailing `{` removed — a
/// compact signature (e.g. `pub fn parse(s: &str) -> Result<T>`).
fn signature_line(node: Node, src: &str) -> Option<String> {
    let text = node.utf8_text(src.as_bytes()).ok()?;
    let first = text.lines().next()?.trim();
    let sig = first.trim_end_matches('{').trim_end();
    (!sig.is_empty()).then(|| sig.to_string())
}

/// Walk the tree, collecting a signature for each definition-kind node.
fn extract_signatures(lang: Lang, src: &str) -> Vec<String> {
    let mut parser = Parser::new();
    if parser.set_language(&lang.ts_language()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(src, None) else {
        return Vec::new();
    };
    let kinds: HashSet<&str> = lang.def_kinds().iter().copied().collect();
    let mut sigs = Vec::new();
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if kinds.contains(node.kind()) {
            if let Some(sig) = signature_line(node, src) {
                sigs.push(sig);
            }
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    sigs
}

/// Lowercase alphanumeric tokens (length ≥ 3) in order, repeats kept, split on
/// non-alphanumerics and camelCase/snake_case boundaries — for lexical ranking.
pub(crate) fn token_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && prev_lower && cur.len() >= 3 {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(ch.to_ascii_lowercase());
            prev_lower = ch.is_lowercase();
        } else {
            if cur.len() >= 3 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            prev_lower = false;
        }
    }
    if cur.len() >= 3 {
        out.push(cur);
    }
    out
}

/// Largest recency boost; relevance (BM25) dominates, recency orders near-ties.
const MAX_RECENCY: f64 = 0.5;
/// Commits of history read for git recency.
const GIT_LOG_COMMITS: usize = 300;

/// Recency boost per repo-relative path, in `[0, MAX_RECENCY]`.
enum Recency {
    Git(HashMap<String, f64>),
    /// (oldest, newest) mtime across the ranked files.
    Mtime(u64, u64),
}

impl Recency {
    fn boost(&self, f: &FileSymbols) -> f64 {
        match self {
            Recency::Git(m) => m.get(&f.path).copied().unwrap_or(0.0),
            Recency::Mtime(oldest, newest) if newest > oldest => {
                MAX_RECENCY * (f.mtime.saturating_sub(*oldest)) as f64 / (newest - oldest) as f64
            }
            Recency::Mtime(..) => 0.0,
        }
    }
}

/// Shell script printing uncommitted + untracked paths, then the recent log as
/// `@`-separated commits (newest first), all relative to the cwd. Runs inside
/// the sandbox: `.git/config` is writable by agents, and options like
/// `core.fsmonitor` or `gpg.program` would otherwise run repo-chosen commands on
/// the host. The `-c` overrides are defense in depth on top of that.
fn git_recency_script() -> String {
    format!(
        "G='git --no-pager -c core.fsmonitor=false -c log.showSignature=false -c core.quotepath=false'\n\
         $G rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 3\n\
         echo '@@dirty'\n\
         $G diff --name-only --relative --no-ext-diff --no-textconv HEAD 2>/dev/null\n\
         $G ls-files --others --exclude-standard 2>/dev/null\n\
         echo '@@log'\n\
         $G log --no-merges -n {GIT_LOG_COMMITS} --name-only --relative --no-renames \
         --no-ext-diff --no-textconv --format=tformat:@ 2>/dev/null\n\
         echo '@@end'\n"
    )
}

/// Parse [`git_recency_script`] output into per-path boosts: uncommitted files
/// get the full boost, committed ones `MAX / (1 + k/10)` for the newest commit
/// `k` (0 = HEAD) touching them. `None` if the output is incomplete.
fn parse_git_recency(out: &str) -> Option<HashMap<String, f64>> {
    let body = out.split_once("@@dirty\n")?.1;
    let (dirty, rest) = body.split_once("@@log\n")?;
    let (log, _) = rest.split_once("@@end")?;
    let mut map = HashMap::new();
    for p in dirty.lines().map(str::trim).filter(|l| !l.is_empty()) {
        map.insert(p.to_string(), MAX_RECENCY);
    }
    let mut commit: i64 = -1;
    for line in log.lines().map(str::trim) {
        if line == "@" {
            commit += 1;
        } else if !line.is_empty() && commit >= 0 {
            let boost = MAX_RECENCY / (1.0 + commit as f64 / 10.0);
            map.entry(line.to_string()).or_insert(boost);
        }
    }
    (!map.is_empty()).then_some(map)
}

/// Git-history recency for files under `repo_root`, or `None` when it isn't a
/// git work tree or git / the sandbox is unavailable.
fn git_recency(repo_root: &Path) -> Option<HashMap<String, f64>> {
    use flows_tools::sandbox::{run_confined, Confinement};
    let c = Confinement {
        offline: true,
        ..Confinement::default()
    };
    let out = run_confined(&git_recency_script(), repo_root, 15, 4 << 20, &c).ok()?;
    parse_git_recency(&out)
}

/// Extract symbols for every supported file, then rank by relevance to `idea`
/// (highest first). Shared by the rendered map and the deterministic eval.
fn collect_ranked(repo_root: &Path, idea: &str) -> Vec<FileSymbols> {
    let mut files: Vec<FileSymbols> = Vec::new();
    for (rel, abs) in source_files(repo_root) {
        let Some(lang) = abs
            .extension()
            .and_then(|e| e.to_str())
            .and_then(Lang::from_ext)
        else {
            continue;
        };
        let Ok(src) = std::fs::read_to_string(&abs) else {
            continue;
        };
        let signatures = extract_signatures(lang, &src);
        if signatures.is_empty() {
            continue;
        }
        let mtime = std::fs::metadata(&abs)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        files.push(FileSymbols {
            path: rel,
            signatures,
            mtime,
            src,
        });
    }
    let recency = match git_recency(repo_root) {
        Some(m) => Recency::Git(m),
        None => Recency::Mtime(
            files.iter().map(|f| f.mtime).min().unwrap_or(0),
            files.iter().map(|f| f.mtime).max().unwrap_or(0),
        ),
    };
    let docs: Vec<String> = files
        .iter_mut()
        .map(|f| format!("{} {}", f.path, std::mem::take(&mut f.src)))
        .collect();
    let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    let relevance = crate::retrieval::bm25_scores(idea, &refs);
    let mut scored: Vec<(f64, FileSymbols)> = files
        .into_iter()
        .zip(relevance)
        .map(|(f, r)| (r + recency.boost(&f), f))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored.into_iter().map(|(_, f)| f).collect()
}

/// Ranked repo-relative file paths (most relevant first) — for the retrieval eval.
pub fn rank_files(repo_root: &Path, idea: &str) -> Vec<String> {
    collect_ranked(repo_root, idea)
        .into_iter()
        .map(|f| f.path)
        .collect()
}

/// Build a token-bounded repo map ranked by relevance to `idea`. Returns an empty
/// string when nothing parses. Appends a partial marker when files were dropped to
/// fit `max_tokens`.
pub fn build_repo_map(repo_root: &Path, idea: &str, max_tokens: usize) -> String {
    let files = collect_ranked(repo_root, idea);
    if files.is_empty() {
        return String::new();
    }

    let mut out = String::from("# Repo map (symbols)\n\n");
    let mut used = est_tokens(&out);
    for (included, f) in files.iter().enumerate() {
        let mut block = format!("### {}\n", f.path);
        for sig in &f.signatures {
            block.push_str(&format!("- {sig}\n"));
        }
        block.push('\n');
        let cost = est_tokens(&block);
        if used + cost > max_tokens && included > 0 {
            out.push_str(&format!(
                "_(map truncated to a token budget; {included} of {} files shown — use `search_code` for the rest)_\n",
                files.len()
            ));
            break;
        }
        out.push_str(&block);
        used += cost;
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn extracts_rust_and_python_signatures() {
        let rs = extract_signatures(
            Lang::Rust,
            "pub fn add(a: i32, b: i32) -> i32 {\n a+b \n}\nstruct Point { x: i32 }\n",
        );
        assert!(rs.iter().any(|s| s.contains("fn add")));
        assert!(rs.iter().any(|s| s.starts_with("struct Point")));
        let py = extract_signatures(
            Lang::Python,
            "def greet(name):\n    return name\nclass Dog:\n    pass\n",
        );
        assert!(py.iter().any(|s| s.starts_with("def greet")));
        assert!(py.iter().any(|s| s.starts_with("class Dog")));
    }

    #[test]
    fn tokenize_splits_camel_and_snake() {
        let t = token_list("resolve_llm dialectEnvKey");
        assert_eq!(t, ["resolve", "llm", "dialect", "env", "key"]);
    }

    #[test]
    fn map_ranks_relevant_file_first_and_bounds_budget() {
        let d = tempdir().unwrap();
        let r = d.path();
        std::fs::write(
            r.join("auth.rs"),
            "pub fn validate_token(t: &str) -> bool { true }\n",
        )
        .unwrap();
        std::fs::write(
            r.join("math.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();
        let map = build_repo_map(r, "how are tokens validated?", 10_000);
        let auth = map.find("auth.rs").unwrap();
        let math = map.find("math.rs").unwrap();
        assert!(auth < math, "relevant file should rank first:\n{map}");
        assert!(map.contains("validate_token"));

        // Tiny budget → partial marker.
        let small = build_repo_map(r, "tokens", 12);
        assert!(
            small.contains("truncated to a token budget"),
            "got: {small}"
        );
    }

    #[test]
    fn body_terms_rank_and_common_words_do_not() {
        let d = tempdir().unwrap();
        let r = d.path();
        // Only a function body mentions the outbox; the other files repeat the
        // common words of the idea.
        std::fs::write(
            r.join("net.ts"),
            "export function flush() {\n  // drain the outbox\n  outbox.drain();\n}\n",
        )
        .unwrap();
        for name in ["a", "b", "c", "d"] {
            std::fs::write(
                r.join(format!("{name}.ts")),
                "// the user should see this when the user is ready\nexport function show() {}\n",
            )
            .unwrap();
        }
        let ranked = rank_files(r, "the user should see the outbox when online");
        assert_eq!(ranked[0], "net.ts", "{ranked:?}");
    }

    #[test]
    fn parses_git_recency_output() {
        let out =
            "@@dirty\nsrc/wip.rs\nnew.rs\n@@log\n@\n\nsrc/a.rs\n@\n\nsrc/b.rs\nsrc/a.rs\n@@end\n";
        let m = parse_git_recency(out).unwrap();
        assert_eq!(m["src/wip.rs"], MAX_RECENCY);
        assert_eq!(m["new.rs"], MAX_RECENCY);
        assert_eq!(m["src/a.rs"], MAX_RECENCY); // newest commit wins
        assert!(m["src/b.rs"] < m["src/a.rs"]);
        // Not a work tree / failed: no markers.
        assert!(parse_git_recency("\n[exit code: 3]").is_none());
        assert!(parse_git_recency("@@dirty\n@@log\n").is_none());
    }

    #[test]
    fn mtime_fallback_spans_the_age_range() {
        let f = |mtime| FileSymbols {
            path: String::new(),
            signatures: vec![],
            mtime,
            src: String::new(),
        };
        let r = Recency::Mtime(1_000, 2_000);
        assert_eq!(r.boost(&f(2_000)), MAX_RECENCY);
        assert_eq!(r.boost(&f(1_000)), 0.0);
        assert_eq!(r.boost(&f(1_500)), MAX_RECENCY / 2.0);
        assert_eq!(Recency::Mtime(5, 5).boost(&f(5)), 0.0);
    }

    fn git(repo: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn git_history_orders_ties_and_dirty_files_first() {
        if !flows_tools::sandbox_available() {
            eprintln!("skipping: no sandbox");
            return;
        }
        let d = tempdir().unwrap();
        let r = d.path();
        git(r, &["init", "-q"]);
        let a = "pub fn alpha() {}\n";
        std::fs::write(r.join("a.rs"), a).unwrap();
        git(r, &["add", "-A"]);
        git(r, &["commit", "-qm", "a"]);
        std::fs::write(r.join("b.rs"), "pub fn beta() {}\n").unwrap();
        git(r, &["add", "-A"]);
        git(r, &["commit", "-qm", "b"]);
        // Rewriting a.rs unchanged makes its mtime newest without a diff.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(r.join("a.rs"), a).unwrap();
        assert_eq!(rank_files(r, "unrelated idea"), ["b.rs", "a.rs"]);

        // An uncommitted edit outranks every commit.
        std::fs::write(r.join("a.rs"), "pub fn alpha2() {}\n").unwrap();
        assert_eq!(rank_files(r, "unrelated idea"), ["a.rs", "b.rs"]);
    }

    #[test]
    fn git_runs_inside_the_sandbox() {
        if !flows_tools::sandbox_available() {
            eprintln!("skipping: no sandbox");
            return;
        }
        let d = tempdir().unwrap();
        let r = d.path();
        git(r, &["init", "-q"]);
        std::fs::write(r.join("a.rs"), "pub fn alpha() {}\n").unwrap();
        git(r, &["add", "-A"]);
        git(r, &["commit", "-qm", "a"]);
        // A hostile hook in .git/config, which an agent could have written. Our
        // `-c` override disables it; even if it ran, the sandbox keeps its write
        // inside the repo, so the marker outside must never appear.
        let outside = tempdir().unwrap();
        let marker = outside.path().join("pwned");
        git(
            r,
            &[
                "config",
                "core.fsmonitor",
                &format!("touch {} #", marker.display()),
            ],
        );
        assert!(git_recency(r).is_some());
        assert!(!marker.exists());
        // Control: unconfined, the same repo does run the hook.
        git(r, &["diff", "--name-only", "HEAD"]);
        assert!(
            marker.exists(),
            "control: fsmonitor hook should fire on the host"
        );
    }
}
