#!/usr/bin/env bash
# Run test/no_preload/refused.sql against a throwaway cluster, restarted between its
# steps with and without pg_tviews in shared_preload_libraries (pg_tviews and
# jsonb_delta must be installed for $PGBIN).
#
#   PGBIN=/usr/lib/postgresql/18/bin test/no_preload/run.sh
#
# NO_PRELOAD_PORT picks the port (default 5498).

set -euo pipefail
PGBIN="${PGBIN:?set PGBIN to the PostgreSQL bin directory}"
PORT="${NO_PRELOAD_PORT:-5498}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dir="$(mktemp -d)"
cleanup() {
    "$PGBIN/pg_ctl" -D "$dir/data" -m immediate stop >/dev/null 2>&1 || true
    rm -rf "$dir"
}
trap cleanup EXIT

"$PGBIN/initdb" -D "$dir/data" -U postgres -A trust >/dev/null

# step <name> <shared_preload_libraries>
step() {
    "$PGBIN/pg_ctl" -D "$dir/data" -m fast stop >/dev/null 2>&1 || true
    "$PGBIN/pg_ctl" -D "$dir/data" \
        -o "-p $PORT -k $dir -c listen_addresses='' -c shared_preload_libraries='$2'" \
        -l "$dir/server.log" -w start >/dev/null
    local out
    out="$("$PGBIN/psql" -X -h "$dir" -p "$PORT" -U postgres -d postgres \
        -v ON_ERROR_STOP=1 -v step="$1" -f "$here/refused.sql" 2>&1)" || {
        echo "$out"
        echo "FAIL: step $1, see above"
        exit 1
    }
    grep -q "no-preload $1: PASS" <<<"$out" || { echo "$out"; exit 1; }
}
step install ''
step setup pg_tviews
step unloaded ''
echo "no-preload: PASS"
