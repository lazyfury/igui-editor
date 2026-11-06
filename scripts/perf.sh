#!/usr/bin/env bash
# Local performance gate — intentionally NOT wired into GitHub Actions.
#
#   scripts/perf.sh                 # compare against target/perf/*.baseline.txt
#   scripts/perf.sh --update        # (re)record both baselines on this machine
#   PERF_THRESHOLD=10 scripts/perf.sh
#
# Steps:
#   1. compile the optional runtime profiler and run its unit test
#   2. run the upstream igui CPU pipeline benches against a saved baseline
#   3. run this app's Markdown/editor bench and compare the key timings
#
# Baselines live under `target/perf/` (build output, not committed): they are
# machine- and load-sensitive, so a CI runner would not produce a comparable
# number. Keep this local; `scripts/check.sh` stays the correctness gate.
set -euo pipefail
cd "$(dirname "$0")/.."

threshold="${PERF_THRESHOLD:-15}"
repeats="${PERF_REPEATS:-3}"
perf_dir="${PERF_DIR:-target/perf}"
update=0
for arg in "$@"; do
    case "$arg" in
        --update) update=1 ;;
        --help | -h)
            sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "perf.sh: unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

step() { printf '\n==> %s\n' "$*"; }
mkdir -p "$perf_dir"
# The upstream bench binary runs with the igui workspace as its working
# directory, so baselines must be absolute.
perf_dir="$(cd "$perf_dir" && pwd)"

step "profiler build (feature=profile)"
cargo test --features profile --no-default-features profile::tests

step "upstream CPU pipeline (igui_bench_suite)"
upstream_base="$perf_dir/igui-cpu.baseline.txt"
if (( update )) || [[ ! -f "$upstream_base" ]]; then
    cargo bench --manifest-path ../igui/Cargo.toml -p igui_bench_suite --bench pipeline \
        -- --save-baseline "$upstream_base"
    echo "saved baseline: $upstream_base"
else
    cargo bench --manifest-path ../igui/Cargo.toml -p igui_bench_suite --bench pipeline \
        -- --baseline "$upstream_base" --threshold "$threshold"
fi

step "editor Markdown/typography bench (best of $repeats)"
labels=(
    "markdown parse"
    "preview layout"
    "minimap build"
    "minimap incremental"
    "typing, map off"
    "typing, map on"
    "preview paint"
    "typing (16 ms frame)"
)

metric() {
    # Time value immediately before the first `ms/iter` or `ms/frame` token.
    awk -v label="$1" '
        index($0, label) == 1 {
            for (i = 2; i <= NF; i++) {
                if ($i ~ /^ms\/(iter|frame)$/) { print $(i - 1); exit }
            }
        }
    ' "$2"
}

# One 20-iteration sample is sensitive to scheduling/frequency noise. Run the
# app bench a few times and keep the best value per label (standard for a
# machine-local microbench); the upstream suite does its own warmup/sampling.
for ((run = 1; run <= repeats; run++)); do
    scripts/bench.sh >"$perf_dir/markdown-$run.txt"
done
cat "$perf_dir/markdown-1.txt"

app_out="$perf_dir/markdown.txt"
app_base="$perf_dir/markdown.baseline.txt"
: >"$app_out"
for label in "${labels[@]}"; do
    best=""
    for ((run = 1; run <= repeats; run++)); do
        value=$(metric "$label" "$perf_dir/markdown-$run.txt")
        [[ -z "$value" ]] && continue
        if [[ -z "$best" ]] || awk -v a="$value" -v b="$best" 'BEGIN { exit !(a < b) }'; then
            best="$value"
        fi
    done
    if [[ -n "$best" ]]; then
        printf '%s %s ms/iter\n' "$label" "$best" >>"$app_out"
    fi
done

if (( update )) || [[ ! -f "$app_base" ]]; then
    cp "$app_out" "$app_base"
    echo "saved baseline: $app_base"
    exit 0
fi

status=0
for label in "${labels[@]}"; do
    base=$(metric "$label" "$app_base")
    current=$(metric "$label" "$app_out")
    if [[ -z "$base" || -z "$current" ]]; then
        printf '  %-24s skipped (benchmark missing)\n' "$label"
        continue
    fi
    if ! awk -v label="$label" -v base="$base" -v current="$current" -v threshold="$threshold" '
        BEGIN {
            limit = base * (1 + threshold / 100)
            if (current > limit) {
                printf "  %-24s REGRESSION  %.3f -> %.3f ms (limit %.3f)\n", label, base, current, limit
                exit 1
            }
            printf "  %-24s ok          %.3f -> %.3f ms\n", label, base, current
        }
    '; then
        status=1
    fi
done

if (( status != 0 )); then
    printf '\nperformance regression over %s%% threshold\n' "$threshold" >&2
    printf 'rerun with --update only after confirming the change is intentional\n' >&2
fi
exit "$status"
