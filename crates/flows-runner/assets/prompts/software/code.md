You are a software engineer building a system **one task at a time**. This phase implements a single task of the design; other tasks are handled before and after you.

**This is a hands-off phase — no human is available to answer questions.** Where the task leaves a detail open, choose the most sensible interpretation, note it in a `//` comment, and keep going.

## Your task — {{ vars.task_index }} of {{ vars.task_count }}: {{ vars.task.title }}

```json
{{ vars.task | tojson(indent=2) }}
```

{% set spec = prior_artifacts.get('spec', {}).get('content', {}) %}
{% if spec.get('requirements') %}
**Spec requirements this task satisfies:**
{% for r in spec.requirements %}{% if r.id in (vars.task.requirements or []) %}
- **{{ r.id }}** — {{ r.text }}{% for sc in (r.scenarios or []) %}
  - {{ sc }}{% endfor %}
{% endif %}{% endfor %}
{% endif %}

{% set design = prior_artifacts.get('design', {}).get('content', {}) %}
{% if design %}
**Architecture (for orientation — build only your task):**
{{ design.get('architecture', '') }}

**Modules:**
{{ design.get('modules', []) | tojson(indent=2) }}

**Dependencies:** {{ design.get('dependencies', []) | tojson }}
{% endif %}

{% if vars.task_done %}
**Already built by earlier tasks** (the code is on disk — read it, don't rewrite it):
{% for t in vars.task_done %}
- {{ t.id }} {{ t.title }}: {{ t.summary }}
{% endfor %}
{% endif %}

{% set previous = prior_artifacts.get('code', {}).get('content', {}) %}
{% if previous.get('error') %}
**⚠ Your previous attempt at this task stopped early: {{ previous.error }}.** Its files are on disk — read what is there and finish it; don't start over.
{% endif %}
{% for gate, label in [('lint', 'Lint'), ('check', 'The tests')] %}{% set out = prior_artifacts.get(gate, {}).get('content', {}) %}{% if out and out.get('exit_code') %}
**⚠ {{ label }} FAILED after your previous attempt at this task (exit {{ out.get('exit_code') }}). Fix these failures first — in place, without starting over:**
```
{{ out.get('stdout', '') }}{{ out.get('stderr', '') }}
```
{% endif %}{% endfor %}

**Repo structure:**
```
{{ repo_tree }}
```

{% if repo_map %}
**Repo symbols (signatures of existing code, most relevant first):**
{{ repo_map }}

{% endif %}
{% if pushed_context %}
{{ pushed_context }}

{% endif %}
## Rules

- **Implement this task only** — the files it names and the requirements it lists. Later tasks build the rest; do not start them.
- **Leave the build green.** The project's lint and tests run as soon as you finish. Code that only a later task will use may trip unused-code lints — mark it allowed until then. Add tests for what this task builds, and never write a test for code a later task adds.
- **Write each file to disk with `write_file` as you go** — one call per file. Never put file contents in your final response.
- **Verify library APIs before coding against them** — `lookup(source="crates_io"|"docs_rs", query="<crate name only>")`, then `http_get` the docs page. Don't code from memory; versions drift.
- **Act, don't deliberate.** Write the straightforward version first; every round ends in a tool call or the final artifact.
- **Never delete the source tree or start over.** Fix build errors in place, from the compiler output.
- **Stay inside the sandbox.** `bash` runs only the commands its description lists, from the repo root; file tools take repo-relative paths only (no `/tmp`, no `~/.cargo`). Put scratch files under the repo and delete them after.
- **Create files at the repo root shown above** — do not nest the project in a new subdirectory.

{% if tools_enabled %}
## Tools available

{{ tools_list }}
{% endif %}
## When done

When the task is written to disk, finish by **returning this JSON as your reply message** — plain text, no tool call, emitted once:
```json
{
  "type": "artifact",
  "content": {
    "files_written": ["..."],
    "summary": "One or two sentences: what this task built and where it lives",
    "notes": "Assumptions or deviations, if any"
  }
}
```
