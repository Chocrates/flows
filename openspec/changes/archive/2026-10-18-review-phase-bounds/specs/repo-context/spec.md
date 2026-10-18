## ADDED Requirements

### Requirement: Uncommitted changes in the prompt

Prompts SHALL receive a `changes` variable: the untracked files by name, then the uncommitted diff against `HEAD`, capped at 20,000 characters with a truncation note. It SHALL be empty outside a git repository, before the first commit, and when nothing has changed. The fallback prompt and the feature `review` prompt SHALL show it when it is non-empty.

#### Scenario: Review sees the implementation

- **WHEN** implement edited `cron.go` and added `cron_last_test.go` without committing
- **THEN** the review prompt lists `cron_last_test.go` and shows the `cron.go` diff.

#### Scenario: Large diff

- **WHEN** the diff is longer than the cap
- **THEN** the new files are still listed and the diff ends with a truncation note.
