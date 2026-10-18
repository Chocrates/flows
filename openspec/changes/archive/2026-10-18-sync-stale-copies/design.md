## Decisions

- **A hash record, not timestamps or version stamps in the files.** The copies stay clean, editable YAML and Markdown, and an edit is detected without depending on mtimes.
- **Copies with no record are kept.** Copies made before this change can't be told apart from user edits, so sync never overwrites them without `--force`; doctor flags them instead.
- **Logic in `flows-runner`.** It needs the builtin templates (core) and prompts (runner), and the CLI must not hold business logic.
