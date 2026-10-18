## Decisions

- **Forced reply, not a synthetic "approved".** The runner does not know a phase's verdict schema. Asking the model for its artifact keeps the verdict real: a review that found problems still loops back to implement.
- **3 reserved rounds.** That is one forced reply, plus the restart that follows a tool call written as text, plus one more. Budgets under 6 rounds keep the old behaviour, so small caps used deliberately still fail hard.
- **Applies to write-capable phases too.** An implement phase that runs out its budget hands a summary to lint and test. Those gates catch unfinished work and retry implement, which is better than failing the run outright. Inline `content.files` is only written for models without tools, so a forced reply cannot write files.
- **New files listed before the diff**, so a long diff cannot truncate them away; the model reads them with `read_file`. No flag: like `repo_tree`, it is plain prompt context and costs one `git diff`.
