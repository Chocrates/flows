## Why

In an e2e run, the review phase of a 27B local model got confused by a test that kept failing, decided its edits were not "landing on disk", and ran `rm -rf .flows Makefile README.md cmd go.mod internal`. It then rebuilt only the package under review from memory, approved its own work, and the commit phase committed a change that deleted 2480 lines across 33 files.

The bash allowlist allowed `rm*` so that review could remove scratch files. The safety blocklist only knows a few source directory names (`src`, `lib`, `tests`, ...), and pattern matching on command text cannot be complete anyway: `go run`, `make` and `python -m` can all delete files without the word `rm`. Agents do need to delete files: scratch files they made, and files a refactor leaves with no purpose.

## What Changes

- Every bash call in an agent phase is bracketed by snapshots of the working tree (a git tree object written through a temporary index, so new untracked files are included and the real index is untouched).
- Files created during the phase may be deleted freely. Files that existed when the phase started may be deleted at most 3 per command and 10 per phase. A command over either limit has every file it deleted restored at once, and the model is told why; the call counts as a refused call for the stall guard.
- Outside a git repository there is nothing to restore from, so a command that runs `rm` is refused.
- The feature prompts tell the model when deleting is right (scratch files, files made obsolete by the change), to delete one file at a time, and never to delete and recreate files to start clean. Implement and review artifacts list deleted files with a reason in `files_deleted`.

## Capabilities

### New Capabilities

### Modified Capabilities
- `agent-loop-guards`: deletion guard for shell commands.

## Impact

- `flows-runner`: new `deletion_guard` module, wired into the bash dispatch in `agent.rs`.
- `flows-tools::bash`: exposes whether a command invokes a given program.
- Builtin prompts `feature/implement.md`, `feature/review.md`.
