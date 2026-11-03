#!/usr/bin/env bash
# Single local gate: formatting, lints, unit tests, and the headless pipeline.
#
#   scripts/check.sh            fast gate (default features)
#   scripts/check.sh --release  also build the shipped release binary
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

if [[ "${1:-}" == "--release" ]]; then
    step "cargo build --release --no-default-features"
    cargo build --release --locked --no-default-features
fi

printf '\nAll checks passed.\n'
