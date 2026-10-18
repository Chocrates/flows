## Decisions

- **All scoped prior artifacts, in order.** The fallback cannot know which a phase needs; `scoped_artifacts` already limits them to what the phase may see. Each is capped at 6000 characters of JSON so a script phase's long output cannot swamp the prompt.
- **Same variables as shipped prompts** (`repo_map`, `pushed_context`, `tools_list`, `tools_enabled`), so context flags behave the same for any template.
- **`name/` for directories**, matching `list_files`, so paths copied from the tree are valid repo-relative paths.
