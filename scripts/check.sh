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
        size=$(wc -c < target/release-fast/igui-editor | tr -d ' ')
        printf 'release-fast binary: %s MB (not the shipped size)\n' \
            "$(awk "BEGIN { printf \"%.2f\", $size / 1048576 }")"
    else
        step "cargo build --release --no-default-features"
        cargo build --release --locked --no-default-features
        # "Lightweight" is measured, not asserted: keep the shipped binary from
        # growing silently. The limit is checked only for the fat-LTO release
        # build that packaging ships.
        step "release binary size"
        size=$(wc -c < target/release/igui-editor | tr -d ' ')
        # Size is a soft guard against silent growth, not an optimization goal:
        # the frame/latency budgets in docs/benchmarks.md are the real gate.
        limit=$((12 * 1024 * 1024))
        printf 'release binary: %s MB (limit 12.00 MB)\n' \
            "$(awk "BEGIN { printf \"%.2f\", $size / 1048576 }")"
        if (( size > limit )); then
            printf 'release binary is %s bytes, over the %s-byte budget\n' \
                "$size" "$limit" >&2
            exit 1
        fi
    fi
fi

printf '\nAll checks passed.\n'
