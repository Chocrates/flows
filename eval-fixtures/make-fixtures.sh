#!/usr/bin/env bash
# Regenerate the on-disk eval fixture repos from the live workspace tree.
#
# Snapshots are source-only (no target/.git). After snapshotting, the coder
# fixtures have one or more functions stubbed to `todo!()` so their existing
# tests fail until an agent reimplements them; the reasoner fixtures ship the
# code unchanged (they are graded by rubric, not a verifier).
#
# Run from the workspace root:  ./eval-fixtures/make-fixtures.sh
# The stubbing itself is applied by the companion patches in stubs/*.patch is
# NOT used — stubs are applied inline here to keep it one file.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FX="$ROOT/eval-fixtures"
cd "$ROOT"

# Full source snapshot of the whole workspace into <dest>/repo.
snapshot_workspace() {
  local dest="$1"
  rm -rf "$dest/repo"
  mkdir -p "$dest/repo"
  rsync -a \
    --exclude '.git' --exclude 'target' --exclude 'eval-fixtures' \
    --exclude '.flows' --exclude '.claude' \
    ./ "$dest/repo/"
}

# Single-crate (flows-core) slice into <dest>/repo: a trimmed workspace root +
# the full Cargo.lock + just crates/flows-core.
snapshot_core_slice() {
  local dest="$1"
  rm -rf "$dest/repo"
  mkdir -p "$dest/repo/crates"
  cp -r crates/flows-core "$dest/repo/crates/flows-core"
  rm -rf "$dest/repo/crates/flows-core/target"
  cp Cargo.lock "$dest/repo/Cargo.lock"
  cat > "$dest/repo/Cargo.toml" <<'TOML'
[workspace]
resolver = "2"
members = ["crates/flows-core"]

[workspace.package]
version = "0.2.0"
edition = "2021"
rust-version = "1.80"
license = "MIT"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml_ng = "0.10"
thiserror = "2"
chrono = { version = "0.4", default-features = false, features = ["clock", "serde"] }
TOML
}

# Replace a Rust fn body (identified by its signature line) with `todo!()`.
# Usage: stub_fn <file> <signature-substring>
stub_fn() {
  local file="$1" sig="$2"
  python3 - "$file" "$sig" <<'PY'
import re, sys
path, sig = sys.argv[1], sys.argv[2]
src = open(path).read()
i = src.index(sig)
# find the opening brace of the fn body
b = src.index('{', i)
# walk to the matching close brace
depth, j = 0, b
while j < len(src):
    if src[j] == '{': depth += 1
    elif src[j] == '}':
        depth -= 1
        if depth == 0: break
    j += 1
stub = ' {\n    todo!("implement per the doc comment and the existing tests")\n}'
src = src[:b] + stub + src[j+1:]
open(path, 'w').write(src)
print(f"stubbed: {sig} in {path}")
PY
}

echo "→ whole-workspace (1 stub, easy)"
snapshot_workspace "$FX/whole-workspace"
stub_fn "$FX/whole-workspace/repo/crates/flows-core/src/config.rs" \
  "pub fn parse_context_config(name: &str)"

echo "→ whole-workspace-hard (3 stubs across crates)"
snapshot_workspace "$FX/whole-workspace-hard"
stub_fn "$FX/whole-workspace-hard/repo/crates/flows-core/src/config.rs" \
  "pub fn parse_context_config(name: &str)"
stub_fn "$FX/whole-workspace-hard/repo/crates/flows-runner/src/runner.rs" \
  "pub fn eval_loop_condition(condition: &str"
stub_fn "$FX/whole-workspace-hard/repo/crates/flows-cli/src/main.rs" \
  "fn preflight_llm(llm: &flows_core::models::LLMConfig)"

echo "→ medium-core-coder (1 stub, single crate)"
snapshot_core_slice "$FX/medium-core-coder"
stub_fn "$FX/medium-core-coder/repo/crates/flows-core/src/models.rs" \
  "pub fn resolve_profile("

echo "→ medium-core-reasoner (no stub — rubric graded)"
snapshot_core_slice "$FX/medium-core-reasoner"

echo "→ large-reasoner (no stub — rubric graded)"
snapshot_workspace "$FX/large-reasoner"

echo "done."
