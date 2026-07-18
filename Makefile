# Standard tooling entrypoints for this Rust workspace. Agent flows (and the
# AGENTS.md policy) expect `make <target>` rather than bare cargo invocations.
.PHONY: test lint fmt fmt-check build check dev install

# Build + install the `flows` CLI to ~/.cargo/bin (quiet).
install:
	cargo install -q --path crates/flows-cli

test:
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

build:
	cargo build --workspace

# Full gate: format, lint, test.
check: fmt-check lint test

dev:
	@echo "No dev server for this workspace."
