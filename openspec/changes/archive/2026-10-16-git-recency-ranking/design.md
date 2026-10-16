## Context

`repo_map::collect_ranked` scores each file as `overlap + recency`, where `recency = 0.5 * mtime / newest_mtime`. With Unix-second mtimes that ratio is ≈1.0 for every file, so the boost never separates anything.

## Goals / Non-Goals

**Goals:** a recency signal that actually varies; safe to run on a repo an agent has been writing to; no new config.

**Non-Goals:** churn/frequency weighting, blame-level granularity, ranking retrieval chunks (retrieval has its own scorer).

## Decisions

- **Rank by commit position, not timestamp.** Read `git log --no-merges -n 300 --name-only --relative --format=@%ct`. A file's boost is `0.5 / (1 + k/10)` where `k` is the index of the newest commit touching it (0 = HEAD). Uncommitted files (`git diff --name-only HEAD --relative` plus untracked from `git ls-files --others --exclude-standard`) get the full 0.5. Files outside the window get 0. Position is robust to clock skew and rebases; the window keeps it cheap on large histories.
- **Run git in the sandbox.** The repo root is writable by sandboxed commands, so `.git/config` is attacker-controllable. `git log`/`git diff` can run `core.fsmonitor`, `gpg.program` (via `log.showSignature`), external diff drivers, and textconv. Running the commands through `sandbox::run_confined` with `offline: true` confines any of that like any other agent command. As defense in depth the invocation also overrides `core.fsmonitor=false`, `log.showSignature=false`, `core.quotepath=false`, and passes `--no-ext-diff --no-textconv`.
- **Fallback.** No sandbox, not a git repo, non-zero exit, or empty history → mtime, scaled `0.5 * (mtime - oldest) / (newest - oldest)` (0 when all equal).
- **One shell call.** All three git commands run in one `sh -c` with section markers, so a map costs one sandbox spawn.

## Risks / Trade-offs

- A sandbox spawn per map build (~tens of ms). Maps are built once per agent phase.
- Single-commit repos (e.g. eval fixtures) get a uniform boost; ranking falls back to pure relevance, which matches today's effective behavior.
