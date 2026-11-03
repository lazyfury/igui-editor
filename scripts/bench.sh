#!/usr/bin/env bash
# Markdown parsing/layout/paint benchmarks (release-optimized bench profile).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo bench --features bench --bench markdown "$@"
