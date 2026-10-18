## Why

A copy of a builtin under `~/.flows/` takes precedence over the builtin, and `templates sync` never touched an existing copy. An e2e run showed the cost: the review-prompt fix existed in the builtin but never reached the model, because `~/.flows/prompts/feature/review.md` was a stale copy from an older build. Nothing told the user.

## What Changes

- `templates sync` records the blake3 hash of every file it writes in `~/.flows/synced.json`. On a rerun it updates copies that still have their recorded hash (unedited), keeps edited copies and copies with no record, and lists what it kept. `--force` replaces everything, as before.
- `flows doctor` lists copies that are outdated (unedited, builtin changed), copies with no record that differ from the builtin (possibly stale), and customized ones.
- The sync logic moves from the CLI into `flows_runner::sync`.

## Capabilities

### New Capabilities

### Modified Capabilities
- `template-registry`: synced builtin copies are tracked and updated.

## Impact

- New `flows-runner::sync`; `flows-cli` `templates sync` and `doctor`.
