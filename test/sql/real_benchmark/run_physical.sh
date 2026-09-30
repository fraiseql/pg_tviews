#!/usr/bin/env bash
# Full physical-cost matrix: the product benchmark (run.sh) plus every
# scenarios/*.sh, all into one run directory, then render report.md.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./run_physical.sh [--scales "small medium"]
#
# Env: RUN_DIR (default results/physical/<timestamp>), MODES ("unlogged logged"),
#      EXPLAIN (1), SCALE (skewed_fanout, 1), plus each scenario's own knobs.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCALES="small medium"
[[ "${1:-}" == "--scales" ]] && SCALES="$2"
RUN_DIR="${RUN_DIR:-$here/results/physical/$(date +%Y%m%d-%H%M%S)}"
export RUN_DIR

"$here/run.sh" --scales "$SCALES" || exit 1
for s in "$here"/scenarios/*.sh; do
  "$s" || exit 1
done
python3 "$here/aggregate.py" physical "$RUN_DIR" -o "$RUN_DIR/report.md"
