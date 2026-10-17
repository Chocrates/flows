## Why

Templates can only be shared by copying YAML and prompt files around. The roadmap asks for installing templates from elsewhere. A template is code: script phases run shell commands on the host unsandboxed, and agent phases choose tool permissions. Installing one from the internet needs pinning, review, and confinement.

## What Changes

- `flows templates install <git-url>[//<path>][@<ref>] [--as <name>] [--trust] [--yes]`: clones the repo, resolves the ref to a commit SHA, finds one template (`<path>` is a `.yml` file, or a directory with exactly one), and collects its prompts (`prompts/<stem>/*.md` next to it).
- Before writing anything, prints a review: source and commit, phases, every script command, bash allowlists and `unrestricted` / `expose_credentials` / `offline` settings, includes, prompt files. Installs only after `y` (or `--yes`).
- Installs globally: `~/.flows/templates/<name>.yml` and `~/.flows/prompts/<name>/`. The lockfile `~/.flows/templates/registry.lock` records source, path, requested ref, commit, file hashes, and trust. Refuses a name that already exists (a builtin or a non-registry template) unless `--as`.
- `flows templates installed`, `flows templates update <name>` (fetch, show a diff of changed files, confirm, re-pin), `flows templates remove <name>`, `flows templates trust <name> [--revoke]`.
- **Untrusted installed templates are confined.** Their script phases run in the shell sandbox (repo-only writes, credentials hidden, host env cleared) instead of directly on the host, and their agent phases cannot set `expose_credentials`. This applies wherever the phases appear, including via `include:`. Trusting a template lifts both restrictions. Local and builtin templates are unchanged, and a project template of the same name still overrides an installed one.
- `flows plan` marks confined phases.

## Capabilities

### New Capabilities
- `template-registry`: install, review, pin, update, remove, trust.

### Modified Capabilities
- `security-model`: script phases from untrusted installed templates are sandboxed.

## Impact

New `flows-core/src/registry.rs`; `PhaseConfig.untrusted`; template loading marks installed phases; `flows-tools` exposes a prepared confined `Command`; `script.rs` uses it; agent bash confinement; CLI subcommands; docs.
