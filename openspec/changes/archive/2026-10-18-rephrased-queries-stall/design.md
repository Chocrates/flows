## Decisions

- **Word-set overlap (Jaccard ≥ 0.5), not result comparison.** Results differ between rewordings, which is why the identical-result check missed them. Comparing the queries themselves catches the loop: 28 of the 32 real queries are flagged, while unrelated queries share no words.
- **Counted as a stall, not refused.** A rephrasing can still help once in a while, so the call runs. Only a run of rephrasings trips the existing nudge-then-force sequence.
- **Source ignored.** The same question asked of github and then stackexchange is still the same question.
- **The working tree as evidence for files_written.** The guard exists to catch code inlined into a reply into an empty or untouched repo. If the tree has uncommitted changes, the claimed work exists. Outside git the guard behaves as before.
