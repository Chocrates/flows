## Why

The sandbox binds `/` read-only, so a sandboxed command can still *read* anything the user can: `~/.ssh` private keys, `~/.aws`, `~/.config/gh` tokens, browser profiles, password stores — and `~/.flows/config.yml`, which holds the user's LLM API keys. The network is shared, so a prompt-injected agent with any interpreter on its allowlist can read a key and send it out. `HOME=/tmp` only hides dotfiles from tools that resolve paths through `$HOME`; an absolute path still works.

## What Changes

- Sandboxed commands (bash tool and plugin tools) run with a fixed list of credential locations under the user's home hidden: directories are covered by an empty tmpfs, files by `/dev/null` (bwrap); Seatbelt denies reads of them.
- `~/.flows/config.yml` and `~/.flows/projects/` (API keys) are always hidden.
- **Opt-out per phase:** `tools.bash.expose_credentials: true` re-exposes the credential list (not flows' own config) for builds that need git-over-ssh or private-registry credentials. Plugin tool YAML accepts the same key.
- `flows doctor` reports how many credential paths are hidden on this host.
- **BREAKING (behavioural):** a sandboxed command that read `~/.ssh` etc. (e.g. `cargo build` with a git-over-ssh dependency, `git fetch` over ssh) now fails until the phase sets `expose_credentials: true`. Script phases are not sandboxed and are unaffected.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `security-model`: new requirement that credential stores are unreadable from the sandbox, with the opt-out.

## Impact

- `flows-tools::sandbox`: `Confinement` (limits + credential exposure) replaces `&Limits` in `run_confined`; the `Sandbox::wrap` backend hook takes the hidden-path list; bwrap and Seatbelt apply it.
- `flows-core`: `BashConfig.expose_credentials`.
- `flows-tools::plugin`: `expose_credentials` key.
- `flows-cli` doctor line; docs (templates, security notes).
