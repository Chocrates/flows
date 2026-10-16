## 1. Git recency

- [x] 1.1 Git history reader in `repo_map.rs`: one sandboxed, offline call; parse commit positions + dirty/untracked files; hardening `-c` overrides
- [x] 1.2 Score with git recency, mtime fallback scaled to the repo's age range
- [x] 1.3 Tests: committed-order tie-break beats mtime, dirty file first, non-git fallback, parser unit tests
- [x] 1.4 `flows eval retrieval` map scores no worse than baseline (MEAN map MRR 0.47); docs + ROADMAP
