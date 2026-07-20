//! Structured lookups against source-specific JSON APIs — crates.io, docs.rs,
//! Stack Exchange, Wikipedia, GitHub. Unlike scraped web search (a single flaky
//! HTML endpoint), these hit stable JSON APIs with no anti-bot walls, which is
//! exactly what a coding agent needs to check library/API details.
//!
//! Extensible by design: implement [`Source`] for a new provider and add one line
//! to [`sources`]. Everything else (the tool schema, dispatch, docs) enumerates
//! the registry, so nothing else needs to change.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{ToolError, ToolResult};

/// One normalized result row, uniform across every source.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// A structured-lookup provider. Add a new one by implementing this and
/// registering it in [`sources`].
#[async_trait]
pub trait Source: Send + Sync {
    /// Stable id used as the `source` argument and in the tool docs.
    fn id(&self) -> &'static str;
    /// One-line description for the tool schema / `tools_list`.
    fn describe(&self) -> &'static str;
    /// Run the query, returning normalized hits (most-relevant first).
    async fn search(
        &self,
        client: &reqwest::Client,
        query: &str,
        max: usize,
    ) -> ToolResult<Vec<Hit>>;
}

/// The registered sources. **Add a provider here** to extend the tool.
pub fn sources() -> Vec<Box<dyn Source>> {
    vec![
        Box::new(CratesIo),
        Box::new(DocsRs),
        Box::new(StackExchange),
        Box::new(Wikipedia),
        Box::new(GitHub),
    ]
}

/// Ids of every registered source, for the tool schema + error messages.
pub fn source_ids() -> Vec<&'static str> {
    sources().iter().map(|s| s.id()).collect()
}

/// `id — description` lines for the tool docs.
pub fn source_catalog() -> String {
    sources()
        .iter()
        .map(|s| format!("- `{}` — {}", s.id(), s.describe()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn client() -> ToolResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        // crates.io / GitHub require a descriptive User-Agent; be a good citizen.
        .user_agent("flows/0.2 structured-lookup (https://github.com/)")
        .build()
        .map_err(|e| ToolError::new(format!("client build failed: {e}")))
}

/// Run a structured lookup against `source`. Returns
/// `{source, query, results: [{title, url, snippet}]}`.
pub async fn lookup(source: &str, query: &str, max: usize) -> ToolResult<Value> {
    let max = if max == 0 { 5 } else { max.min(20) };
    let all = sources();
    let Some(s) = all.iter().find(|s| s.id() == source) else {
        return Err(ToolError::new(format!(
            "unknown source {source:?}. Available: {}",
            source_ids().join(", ")
        )));
    };
    let client = client()?;
    let hits = s.search(&client, query, max).await?;
    Ok(json!({ "source": source, "query": query, "results": hits }))
}

// -- shared HTTP helpers -----------------------------------------------------

/// GET `base` with `params` (percent-encoded by reqwest) and parse JSON.
async fn get_json(
    client: &reqwest::Client,
    base: &str,
    params: &[(&str, &str)],
) -> ToolResult<Value> {
    let mut req = client.get(base);
    if !params.is_empty() {
        req = req.query(params);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ToolError::new(format!("request failed: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ToolError::new(format!("read failed: {e}")))?;
    if !status.is_success() {
        let head: String = text.chars().take(200).collect();
        return Err(ToolError::new(format!("HTTP {status}: {head}")));
    }
    serde_json::from_str(&text).map_err(|e| ToolError::new(format!("bad JSON from {base}: {e}")))
}

/// Strip HTML tags + decode a few entities (Wikipedia snippets are HTML).
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

// -- sources -----------------------------------------------------------------

/// crates.io — Rust crate search (name, description, version, docs link).
struct CratesIo;
#[async_trait]
impl Source for CratesIo {
    fn id(&self) -> &'static str {
        "crates_io"
    }
    fn describe(&self) -> &'static str {
        "Search crates.io for Rust crates (name, latest version, description, docs link)."
    }
    async fn search(&self, c: &reqwest::Client, q: &str, max: usize) -> ToolResult<Vec<Hit>> {
        let per = max.to_string();
        let v = get_json(
            c,
            "https://crates.io/api/v1/crates",
            &[("q", q), ("per_page", &per)],
        )
        .await?;
        let mut hits = Vec::new();
        for cr in v
            .get("crates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = s(cr, "name");
            let ver = cr
                .get("max_stable_version")
                .and_then(Value::as_str)
                .filter(|x| !x.is_empty())
                .or_else(|| cr.get("max_version").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            let url = cr
                .get("documentation")
                .and_then(Value::as_str)
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("https://docs.rs/{name}/latest/{name}/"));
            hits.push(Hit {
                title: format!("{name} {ver}"),
                url,
                snippet: s(cr, "description"),
            });
        }
        Ok(hits)
    }
}

/// docs.rs — resolve a crate name to its API-docs URL (fetch with `http_get`).
/// docs.rs has no JSON search, so we resolve the crate via crates.io and hand
/// back the canonical latest-docs URL to read.
struct DocsRs;
#[async_trait]
impl Source for DocsRs {
    fn id(&self) -> &'static str {
        "docs_rs"
    }
    fn describe(&self) -> &'static str {
        "Resolve a crate to its docs.rs API-documentation URL, then read it with http_get."
    }
    async fn search(&self, c: &reqwest::Client, q: &str, max: usize) -> ToolResult<Vec<Hit>> {
        let per = max.min(5).to_string();
        let v = get_json(
            c,
            "https://crates.io/api/v1/crates",
            &[("q", q), ("per_page", &per)],
        )
        .await?;
        let mut hits = Vec::new();
        for cr in v
            .get("crates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = s(cr, "name");
            if name.is_empty() {
                continue;
            }
            hits.push(Hit {
                title: format!("{name} — docs.rs"),
                url: format!("https://docs.rs/{name}/latest/{name}/"),
                snippet: format!(
                    "Rust API docs for `{name}`. Fetch this URL with http_get to read item \
                     signatures for the current version. {}",
                    s(cr, "description")
                ),
            });
        }
        Ok(hits)
    }
}

/// Stack Exchange (Stack Overflow) — programming Q&A search.
struct StackExchange;
#[async_trait]
impl Source for StackExchange {
    fn id(&self) -> &'static str {
        "stackexchange"
    }
    fn describe(&self) -> &'static str {
        "Search Stack Overflow questions (title, link, score) for programming problems."
    }
    async fn search(&self, c: &reqwest::Client, q: &str, max: usize) -> ToolResult<Vec<Hit>> {
        let size = max.to_string();
        // The API always gzips responses (handled by reqwest's gzip feature).
        let v = get_json(
            c,
            "https://api.stackexchange.com/2.3/search/advanced",
            &[
                ("order", "desc"),
                ("sort", "relevance"),
                ("q", q),
                ("site", "stackoverflow"),
                ("pagesize", &size),
            ],
        )
        .await?;
        let mut hits = Vec::new();
        for it in v
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let score = it.get("score").and_then(Value::as_i64).unwrap_or(0);
            let answered = it
                .get("is_answered")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tags = it
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            hits.push(Hit {
                title: strip_html(&s(it, "title")),
                url: s(it, "link"),
                snippet: format!(
                    "score {score}{} · [{tags}]",
                    if answered { " · answered" } else { "" }
                ),
            });
        }
        Ok(hits)
    }
}

/// Wikipedia — MediaWiki full-text search (general facts / background).
struct Wikipedia;
#[async_trait]
impl Source for Wikipedia {
    fn id(&self) -> &'static str {
        "wikipedia"
    }
    fn describe(&self) -> &'static str {
        "Search Wikipedia (title, article URL, snippet) for general/background facts."
    }
    async fn search(&self, c: &reqwest::Client, q: &str, max: usize) -> ToolResult<Vec<Hit>> {
        let limit = max.to_string();
        let v = get_json(
            c,
            "https://en.wikipedia.org/w/api.php",
            &[
                ("action", "query"),
                ("list", "search"),
                ("srsearch", q),
                ("format", "json"),
                ("srlimit", &limit),
            ],
        )
        .await?;
        let mut hits = Vec::new();
        let arr = v
            .get("query")
            .and_then(|x| x.get("search"))
            .and_then(Value::as_array);
        for it in arr.into_iter().flatten() {
            let title = s(it, "title");
            hits.push(Hit {
                url: format!("https://en.wikipedia.org/wiki/{}", title.replace(' ', "_")),
                snippet: strip_html(&s(it, "snippet")),
                title,
            });
        }
        Ok(hits)
    }
}

/// GitHub — repository search (usage examples, source). Uses `GITHUB_TOKEN` for a
/// higher rate limit if set, otherwise unauthenticated (works, just throttled).
struct GitHub;
#[async_trait]
impl Source for GitHub {
    fn id(&self) -> &'static str {
        "github"
    }
    fn describe(&self) -> &'static str {
        "Search GitHub repositories (full name, URL, description, stars) for code examples."
    }
    async fn search(&self, c: &reqwest::Client, q: &str, max: usize) -> ToolResult<Vec<Hit>> {
        let per = max.to_string();
        let mut req = c
            .get("https://api.github.com/search/repositories")
            .query(&[("q", q), ("per_page", per.as_str())])
            .header("Accept", "application/vnd.github+json");
        if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
            if !tok.is_empty() {
                req = req.header("Authorization", format!("Bearer {tok}"));
            }
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ToolError::new(format!("request failed: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ToolError::new(format!("read failed: {e}")))?;
        if !status.is_success() {
            let head: String = text.chars().take(200).collect();
            return Err(ToolError::new(format!("HTTP {status}: {head}")));
        }
        let v: Value =
            serde_json::from_str(&text).map_err(|e| ToolError::new(format!("bad JSON: {e}")))?;
        let mut hits = Vec::new();
        for it in v
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let stars = it
                .get("stargazers_count")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            hits.push(Hit {
                title: s(it, "full_name"),
                url: s(it, "html_url"),
                snippet: format!("★{stars} · {}", s(it, "description")),
            });
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_exposes_every_source() {
        let ids = source_ids();
        for expected in [
            "crates_io",
            "docs_rs",
            "stackexchange",
            "wikipedia",
            "github",
        ] {
            assert!(ids.contains(&expected), "missing source {expected}");
        }
        let cat = source_catalog();
        for id in &ids {
            assert!(cat.contains(id), "catalog missing {id}");
        }
    }

    #[tokio::test]
    async fn unknown_source_errors_without_network() {
        let err = lookup("nope", "q", 5).await.unwrap_err();
        assert!(err.0.contains("unknown source"), "{}", err.0);
        assert!(
            err.0.contains("crates_io"),
            "should list options: {}",
            err.0
        );
    }

    #[test]
    fn strip_html_decodes_entities() {
        assert_eq!(strip_html("<b>Bevy</b> &amp; ECS"), "Bevy & ECS");
    }
}
