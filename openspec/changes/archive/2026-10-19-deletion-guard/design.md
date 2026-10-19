## Decisions

- **Check the tree, not the command.** A snapshot before and after each bash call catches every way a command can delete a file (`rm`, `find -delete`, a Makefile target, a program run with `go run`). Command filtering stays as a first line, but is no longer what keeps files safe.
- **Snapshot with git plumbing.** Copy the real index to a temp file, `git add -A -- .` into it, `git write-tree`. The copied index keeps git's stat cache, so only changed files are hashed: about 15 ms on a 34-file repo and 17 ms on a 473-file one. Ignored files (build output) are not in the tree, so cleaning them is never counted. The real index and working tree are never touched by the snapshot.
- **Restore with `git restore --source=<tree> --worktree`.** It writes the deleted files back with the contents they had right before the command, including uncommitted edits and new files, and leaves the index alone.
- **"Existing" means present when the phase started.** A phase-start snapshot decides which deletions count. Files the phase itself made are scratch from its point of view; files an earlier phase made (e.g. implement's new files, seen by review) are protected like tracked ones.
- **Restore the whole command, not just the excess.** A command over the limit is treated as a mistake; partial restores would leave a half-deleted tree that is harder for the model to reason about.
- **Limits 3 per command, 10 per phase.** Enough for a refactor that retires a few files; a larger cleanup has to be done one file at a time with a reason each, which is the point. Constants, not config, until a real need shows up.
- **Outside git, refuse `rm`.** No snapshot means no undo, so fail closed for the one command whose purpose is deleting. Other commands still run.
- **Plugin tools are not covered.** They are user-written commands, not model-written, and already sandboxed.
- **Gutting a file with `write_file` is out of scope.** Shrink detection is fuzzier and is left for later.
