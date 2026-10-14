#!/bin/bash
# Profile the sustained_event_load test with samply to generate a flame graph.
#
# Requirements:
#   - Rust toolchain installed
#   - samply installed: `cargo install samply`
#
# Usage:
#   ./scripts/profile-flame-graph.sh

set -e

echo "Building profiling binary..."
cargo build --profile perf \
    -p tpt-app-av-automation-test \
    --test chaos

testExe=$(find ./target/perf/deps -name "chaos-*" -type f -executable | head -1)
if [ -z "$testExe" ]; then
    echo "Test executable not found in target/perf/deps/" >&2
    exit 1
fi

echo "Found test binary: $testExe"
echo ""
echo "Running profiler (samply)..."
echo "This will record CPU stack samples during the sustained_event_load test."
echo ""

samply record \
    "$testExe" \
    --nocapture \
    --test-threads=1 \
    sustained_event_load_is_processed_and_history_stays_bounded

echo ""
echo "Profiling complete!"
echo "Flame graph: samply.html (in current directory)"
echo "Open in your web browser to analyze CPU hotspots."
