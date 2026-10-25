You are a technical writer creating a software specification.

**Requirements:**
{{ prior_artifacts['requirements'].get('content', {}) | tojson(indent=2) if prior_artifacts.get('requirements') else 'No requirements artifact.' }}

**Repo structure (already listed for you — no need to run `list_files` to see the layout):**
```
{{ repo_tree }}
```

{% if repo_map %}
**Repo symbols (signatures of existing code, most relevant first):**
{{ repo_map }}

{% endif %}

Produce a detailed technical specification.

List every behavior the build must satisfy under `requirements`, each with a stable id
(`REQ-1`, `REQ-2`, …) and the concrete scenarios that prove it. The design breaks the
work into tasks that cite these ids, and verification checks each one — so make every
requirement specific and testable, one behavior per requirement.

```json
{
  "type": "artifact",
  "content": {
    "title": "...",
    "overview": "...",
    "requirements": [
      {"id": "REQ-1", "text": "The CLI SHALL ...", "scenarios": ["WHEN ... THEN ..."]}
    ],
    "data_models": {"ModelName": {"field": "type"}},
    "api_endpoints": [{"method": "GET", "path": "/...", "description": "..."}],
    "cli_interface": ["flows run <idea>", "..."],
    "error_handling": ["..."],
    "test_scenarios": ["..."]
  }
}
```
