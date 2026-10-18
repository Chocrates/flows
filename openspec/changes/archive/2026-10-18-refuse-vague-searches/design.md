## Context

The stall guard counts a round as productive when any tool returned something new. Research tools return a different answer for each query, so a model generating junk queries looks productive forever.

## Goals / Non-Goals

**Goals:** stop the observed degenerate loop cheaply and deterministically.

**Non-Goals:** limiting legitimate research volume (the `research` template does many lookups); judging result relevance.

## Decisions

- **Threshold: fewer than 3 alphanumeric characters.** Real queries ("tokio", "cron L") clear it; the observed loop never did. Checked before any network request.
- **Refusal is a tool error**, so the existing `failed_fetch` / dry-search paths handle it: no new counter.
- **Path hint only when the relative path exists in the repo**, so `/etc/passwd` is not answered with "use etc/passwd". Absolute paths are still refused.

## Risks / Trade-offs

- [A model that degenerates with longer junk ("aaa", "aab")] → not caught; would need relevance scoring. Revisit if seen.
