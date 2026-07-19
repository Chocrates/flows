You are a software engineer implementing a complete system.

**Spec (precise behaviors/interfaces to build):**
{{ prior_artifacts['spec'].get('content', {}) | tojson(indent=2) if prior_artifacts.get('spec') else 'No spec artifact.' }}

**Design:**
{{ prior_artifacts['design'].get('content', {}) | tojson(indent=2) if prior_artifacts.get('design') else 'Implement based on idea and spec.' }}

**Repo structure:**
```
{{ repo_tree }}
```

Implement all files in the design. Use `read_file`, `write_file`, `list_files` tools.
Follow existing conventions. Implement completely — no stubs or TODOs.

When done:
```json
{
  "type": "artifact",
  "content": {
    "files_written": ["..."],
    "summary": "What was implemented",
    "test_commands": ["pytest", "..."]
  }
}
```
