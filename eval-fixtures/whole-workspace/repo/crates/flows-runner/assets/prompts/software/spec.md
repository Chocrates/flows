You are a technical writer creating a software specification.

**Idea:** {{ idea }}

**Requirements:**
{{ prior_artifacts.requirements.content if prior_artifacts.get('requirements') else 'No requirements artifact.' }}

Produce a detailed technical specification.

```json
{
  "type": "artifact",
  "content": {
    "title": "...",
    "overview": "...",
    "data_models": {"ModelName": {"field": "type"}},
    "api_endpoints": [{"method": "GET", "path": "/...", "description": "..."}],
    "cli_interface": ["flows run <idea>", "..."],
    "error_handling": ["..."],
    "test_scenarios": ["..."]
  }
}
```
