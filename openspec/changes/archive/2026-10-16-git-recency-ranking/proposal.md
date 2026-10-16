## Why

The repo map breaks ties between equally relevant files with a recency boost taken from file mtime. Mtime is a poor signal: a fresh clone, a branch switch, or a `touch` gives every file nearly the same timestamp, and the boost is computed as `mtime / newest_mtime`, which is ≈1.0 for every file (seconds since 1970 barely differ). The boost is effectively constant. Git history says which files the project is actually working on.

## What Changes

- In a git repository, the repo map's recency boost comes from git: files with uncommitted changes rank as most recent, then files by how recently a commit touched them (within the last few hundred commits).
- Git runs **inside the shell sandbox**, offline: an agent can write to `.git/config` (it's in the writable repo), and options like `core.fsmonitor` would otherwise execute repo-chosen commands on the host.
- Outside git, or when git or the sandbox is unavailable, the boost falls back to mtime, now scaled across the repo's age range instead of a near-constant ratio.
- Relevance (idea ↔ path/symbol token overlap) still dominates; recency stays a boost of at most 0.5.

## Capabilities

### Modified Capabilities
- `repo-context`: the ranked repo map uses git recency as its tie-breaker.

## Impact

`flows-runner/src/repo_map.rs` (new git history reader), docs, ROADMAP. No config changes; the map stays behind `context.repo_map`.
