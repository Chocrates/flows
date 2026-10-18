## ADDED Requirements

### Requirement: Rephrased searches are stalls

A `web_search` or `lookup` call whose query shares at least half of its words (Jaccard over lowercased words) with an earlier query in the same phase SHALL count as a call that brought nothing new for the stall guard. The call SHALL still be made.

#### Scenario: Reworded lookups

- **WHEN** a plan phase looks up "Quartz cron L last day of month semantics" and then "quartz cron L last day of month" and "cron L last day of month meaning" in separate rounds
- **THEN** the later two count toward the stall guard even though their results differ.

### Requirement: Verify-only passes

A write-capable phase whose artifact lists `files_written` without having written any file SHALL fail only when the repository's working tree has no uncommitted changes either.

#### Scenario: Implement after a review loop-back

- **WHEN** implement runs again over files it already changed, only re-runs tests, and reports those files
- **THEN** its artifact is accepted.
