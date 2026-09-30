#!/usr/bin/env bash
# Run test/no_preload/create_or_replace.sql against a throwaway cluster that does
# not preload pg_tviews (pg_tviews and jsonb_delta must be installed for $PGBIN).
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
"$PGBIN/pg_ctl" -D "$dir/data" -o "-p $PORT -k $dir -c listen_addresses=''" \
    -l "$dir/server.log" -w start >/dev/null

out="$("$PGBIN/psql" -X -h "$dir" -p "$PORT" -U postgres -d postgres \
    -v ON_ERROR_STOP=1 -f "$here/create_or_replace.sql" 2>&1)" || {
    echo "$out"
    echo "FAIL: see above"
    exit 1
}
grep -q 'no-preload create_or_replace: PASS' <<<"$out" || { echo "$out"; exit 1; }
echo "no-preload create_or_replace: PASS"
