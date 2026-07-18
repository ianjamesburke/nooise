#!/usr/bin/env bash
# Time a release-build render and report wall-clock plus the realtime
# multiple (rendered seconds / wall seconds). Use this before and after any
# DSP perf change instead of guessing at the gain — cargo run --release adds
# rebuild time on the first invocation, so this always warms the build first.
# Usage: scripts/bench_render.sh [seconds] [seed]   — called by `just bench`
set -euo pipefail

REPO_ROOT=$(git rev-parse --show-toplevel)
cd "$REPO_ROOT"

seconds="${1:-60}"
seed="${2:-42}"
out="$(mktemp -d)/bench.wav"

cargo build --release --quiet

# BSD date (macOS) doesn't support %N for sub-second precision, so use
# python3's high-res clock instead — it's portable to both BSD and GNU hosts.
now() { python3 -c 'import time; print(time.time())'; }

start=$(now)
./target/release/nooise render --seconds "$seconds" --out "$out" --seed "$seed"
end=$(now)

awk -v start="$start" -v end="$end" -v seconds="$seconds" 'BEGIN {
    wall = end - start
    printf "rendered %ss in %.3fs wall (%.1fx realtime)\n", seconds, wall, seconds / wall
}'
rm -f "$out"
