#!/usr/bin/env bash
# Single local gate: formatting, lints, unit tests, and the headless pipeline.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n==> %s\n' "$*"; }

step "cargo fmt --check"
cargo fmt --check

step "cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

step "cargo test"
cargo test

step "cargo run -- --selfcheck"
cargo run --quiet -- --selfcheck

printf '\nAll checks passed.\n'
