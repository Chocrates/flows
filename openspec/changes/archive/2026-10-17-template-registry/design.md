## Decisions

- **Git, pinned to a SHA.** `git clone --quiet <url> <tmp>` then `git checkout --detach <ref>` (default: remote HEAD) and `git rev-parse HEAD`. Run with `core.hooksPath=/dev/null`, `protocol.ext.allow=never`, `GIT_TERMINAL_PROMPT=0`, no submodules. Host git (not the sandbox) so private repos work with the user's credentials; nothing from the clone executes.
- **Lockfile JSON** at `~/.flows/templates/registry.lock`: `{templates: {name: {source, path, ref, commit, files: {relpath: sha256}, trusted, installed_at}}}`. Written atomically.
- **Trust marking at load.** `template_text` reports where text came from. A phase is `untrusted` when the template that defined it (root or `include:` origin) resolved from the global dir *and* is a lock entry with `trusted: false`. Set on `PhaseConfig.untrusted` (serde-skipped when false) so it survives into run meta and background workers.
- **Confinement.** `flows_tools::sandbox::confined_command(cmd, repo, &Confinement) -> Command` returns the fully prepared confined process (cgroup/rlimits, seccomp prepare, env cleared), shared with `run_confined`. `script.rs` uses it for untrusted phases and keeps its cancel polling. Without a sandbox, an untrusted script phase fails with the sandbox's unavailable reason. Agent phases: `Confinement.expose_credentials` forced false when `phase.untrusted`.
- **Review** is a pure function over the template text + prompt list (`registry::review`) so it is unit-tested; the CLI prints it and reads y/N.
- **Update diff**: unified-ish line diff per changed file via a simple LCS on lines (files are small), plus added/removed file lists.
- **Business logic in core**, CLI only parses args, prompts, prints.
