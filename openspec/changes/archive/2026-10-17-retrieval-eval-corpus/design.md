## Decisions

- **Separate `queries.yml`, not more `fixture.yml`s.** Fixtures carry run-eval fields (suite, verify, timeout) that retrieval doesn't need, and one fixture = one repo copy + one index. A corpus amortizes both across many queries. `eval run` ignores folders without `fixture.yml`, so corpora can live under `eval-fixtures/`.
- **`repo:` reuse.** The flows-workspace corpus points at `whole-workspace/repo` instead of duplicating ~2 MB of snapshot.
- **Synthetic repos for other languages.** Vendoring real OSS projects adds licensing and size questions; small synthetic projects (22–30 files, decoy files sharing vocabulary) exercise ranking across Python, TypeScript and Go now. Real-repo corpora can be added later in the same format.
- **Path-free by construction.** Each corpus was checked so no query contains a file basename, directory, or definition name of the repo; this is what the old fixtures lacked.
- **Equal query weight.** A 1-query fixture and a 15-query corpus would otherwise skew the mean toward fixtures.
