## ADDED Requirements

### Requirement: Shell deletions are bounded

In a git repository, the agent loop SHALL snapshot the working tree (tracked and untracked, not ignored files) when a phase with the bash tool starts and around every bash call. A bash call that deletes more than 3 files present at phase start, or that brings the phase's deletions of such files above 10, SHALL have every file it deleted restored to its content from just before the call, and its result SHALL be an error naming the restored files that counts as a refused call for the stall guard. Files created during the phase SHALL NOT count toward either limit. Outside a git repository, a bash call that runs `rm` SHALL be refused.

#### Scenario: Wiping the repository

- **WHEN** a review phase runs `rm -rf cmd internal Makefile` and those paths hold 30 files present at phase start
- **THEN** all 30 files are back on disk with their earlier contents, and the model receives an error saying they were restored.

#### Scenario: Removing scratch files

- **WHEN** a phase writes `debug_test.go` and `tmp.go` and later runs `rm debug_test.go tmp.go`
- **THEN** the command succeeds and nothing is restored.

#### Scenario: Retiring an obsolete file

- **WHEN** a phase runs `rm internal/old_parser.go` on a tracked file
- **THEN** the file stays deleted.

#### Scenario: Not a git repository

- **WHEN** a phase in a directory without git runs `rm notes.txt`
- **THEN** the command is refused without running.
