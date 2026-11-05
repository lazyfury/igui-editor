#!/usr/bin/env bash
# Single local gate: formatting, lints, unit tests, and the headless pipeline.
#
#   scripts/check.sh            fast gate (default features)
#   scripts/check.sh --release  also build the shipped release binary
#   scripts/check.sh --fast     also build with `profiles.release-fast`
#                               (thin LTO, 16 codegen units); implies --release
set -euo pipefail
cd "$(dirname "$0")/.."

release=0
fast=0
for arg in "$@"; do
    case "$arg" in
        --release) release=1 ;;
        --fast) release=1; fast=1 ;;
        *) printf 'unknown argument: %s\n' "$arg" >&2; exit 2 ;;
    esac
done

# Opt-in compile cache: `RUSTC_WRAPPER=sccache scripts/check.sh` (see
# docs/verification.md). Not enabled here: sccache cannot cache the app's
# incremental units and costs ~1 s per rebuild.

step() { printf '\n==> %s\n' "$*"; }

step "cargo fmt --check"
cargo fmt --check

step "cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

step "cargo test"
cargo test

step "cargo run -- --selfcheck"
cargo run --quiet -- --selfcheck

if (( release )); then
    step "cargo clippy (bench feature)"
    cargo clippy --all-targets --features bench -- -D warnings

    if (( fast )); then
        # Same opt levels as the shipped build, cheaper to link: use while
        # iterating on release-only problems. Packaging always uses `release`.
        step "cargo build --profile release-fast --no-default-features"
        cargo build --profile release-fast --locked --no-default-features
    else
        step "cargo build --release --no-default-features"
        cargo build --release --locked --no-default-features
    fi
fi

printf '\nAll checks passed.\n'
