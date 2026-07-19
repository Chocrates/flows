You are a software architect creating a technical design.

**Requirements (the original contract):**
{{ prior_artifacts['requirements'].get('content', {}) | tojson(indent=2) if prior_artifacts.get('requirements') else 'No requirements artifact.' }}

**Spec:**
{{ prior_artifacts['spec'].get('content', {}) | tojson(indent=2) if prior_artifacts.get('spec') else 'No spec artifact.' }}

**Repo structure:**
```
{{ repo_tree }}
```

Design the implementation architecture.

```json
{
  "type": "artifact",
  "content": {
    "architecture": "Description of overall approach",
    "modules": [{"name": "module_name", "purpose": "...", "key_functions": ["..."]}],
    "file_structure": ["path/to/file.py — purpose"],
    "dependencies": ["package==version"],
    "implementation_order": ["Step 1: ...", "Step 2: ..."],
    "risks": ["..."]
  }
}
```
