# On-disk eval fixtures

Large `flows eval` fixtures that are **too big to embed** in the binary. Run them
by pointing `--fixtures` at this directory:

```bash
flows eval run --fixtures ./eval-fixtures --size large
flows eval list --fixtures ./eval-fixtures
```

Each subfolder is one fixture: a `fixture.yml` plus a `repo/` source tree.

## The fixtures

| fixture | suite | size | gran | repo | success signal |
|---|---|---|---|---|---|
| `medium-core-coder` | coder | medium | flow | `flows-core` slice (1 crate) | `cargo test -p flows-core resolve_profile` |
| `medium-core-reasoner` | reasoner | medium | phase | `flows-core` slice (1 crate) | manual grade (rubric) |
| `whole-workspace` | coder | large | flow | full workspace (6 crates) | `cargo test -p flows-core context_config` |
| `whole-workspace-phase` | coder | large | phase | full workspace, same stub as `whole-workspace` | `cargo test -p flows-core context_config` |
| `whole-workspace-hard` | coder | large | flow | full workspace, **3 stubs across crates** | compound `cargo test` (core+runner+cli) |
| `large-reasoner` | reasoner | large | phase | full workspace (6 crates) | manual grade (rubric) |

`whole-workspace` (flow) vs `whole-workspace-phase` (phase) are the **same task +
stub** at two granularities — a direct read on how much the full pipeline helps or
hurts versus a single implement phase.

**Coder** fixtures have one or more functions replaced with `todo!()` — they
compile but panic, so the tests fail until an agent reimplements them; a correct
fix flips `cargo test` to exit 0. **Reasoner** fixtures ship the code unchanged and
ask the model to trace/explain a real feature; there is no verifier, so grade them
with `flows eval review`.

- **Verify runs on the host** (network + `~/.cargo` cache), so compilation works.
- **Cold compile** happens in each trial's fresh temp dir → long timeouts
  (1200–2400s) and minutes per trial, especially the whole-workspace ones. Keep
  `--trials` small. The medium (single-crate) fixtures are much faster.

## Retrieval corpora

`retrieval/` holds path-free retrieval corpora (`queries.yml` + `repo/`), scored by
`flows eval retrieval --corpus eval-fixtures/retrieval`. See [docs/eval.md](../docs/eval.md#retrieval-corpora).
`flows-workspace` reuses `whole-workspace/repo`, so regenerating that snapshot
can move its labels; re-run the eval after `make-fixtures.sh`.

## Regenerating

The repos are generated from the live tree by `make-fixtures.sh` (snapshots +
applies the `todo!()` stubs). Re-run it after the real code changes:

```bash
./eval-fixtures/make-fixtures.sh
```
