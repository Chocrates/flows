## Context

Sandboxing binds `/` read-only and the project read-write (bwrap), or allows all reads and restricts writes (Seatbelt). Env is cleared and `HOME=/tmp`, but files stay reachable by absolute path. Network is shared on purpose (dependency fetches), so read access to secrets is effectively exfiltration access.

## Goals / Non-Goals

**Goals:** make the common credential stores unreadable by default on both backends; keep an explicit per-phase escape hatch; never break the repo or toolchain cache binds.

**Non-Goals:** discovering secrets heuristically (scanning for `*.pem`, `.env` files outside the repo); hiding secrets *inside* the repo (the agent must be able to read the repo); network egress control (separate roadmap item).

## Decisions

- **Denylist of known locations, not a home-wide mask.** Masking all of `$HOME` breaks toolchains installed there (`~/.rustup`, `~/.nvm`, `~/.pyenv`, `~/.local/bin`) that the locator env vars point at. A fixed list is predictable and documented; the cost is that unlisted secrets stay readable.
- **`Confinement { limits, expose_credentials }`** replaces `&Limits` in `run_confined`, so every caller decides explicitly. The backend hook becomes `wrap(repo, command, hidden: &[PathBuf])`; the list is computed once in `sandbox::hidden_paths` from `$HOME`.
- **bwrap:** for each existing path (canonicalized), `--tmpfs P` for directories and `--ro-bind /dev/null P` for files, placed after `--ro-bind / /` and *before* the repo and cache binds. Later binds win and bwrap creates their mount points inside the tmpfs, so a repo under a masked dir still works.
- **Seatbelt:** `(deny file-read* (subpath P))` / `(literal P)` after the broad `(allow file-read*)`. In SBPL the later matching rule wins. This needs to be verified on macOS, together with the IPC change.
- **flows' own secrets always hidden.** Sandboxed commands never need `~/.flows/config.yml` or `~/.flows/projects/`. `~/.flows/tools/` stays visible because plugin commands may call scripts there.
- **Opt-out is all-or-nothing per phase.** Per-path exposure (`expose_credentials: [ssh]`) is more precise, but no current workflow needs it. A boolean keeps config simple, and it can grow into a list later without breaking `true`.
- **Plugins** read `expose_credentials` from their YAML. The plugin author (the user) trusts the plugin, but the model still controls its arguments.

## Risks / Trade-offs

- Builds with git-over-ssh dependencies fail until the phase opts out. The error will be ssh's "Permission denied (publickey)" or "No such file". The docs and `flows doctor` mention the flag.
- The denylist will miss some tools' credential files. Document the list; adding entries is cheap.
