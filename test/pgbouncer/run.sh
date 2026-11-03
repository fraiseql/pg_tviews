#!/usr/bin/env bash
# Run the pgbouncer suite: a throwaway pgbouncer in transaction pooling mode in
# front of the server, and test-2pc.sh through it.
#
# Usage: PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/pgbouncer/run.sh
#   PGBOUNCER   the pgbouncer binary (default: the one on PATH)
#   BOUNCER_PORT  the port it listens on (default 6432)
# The server needs max_prepared_transactions > 0.
set -euo pipefail
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bouncer="${PGBOUNCER:-pgbouncer}"
port="${BOUNCER_PORT:-6432}"
db=pg_tviews_pgbouncer
work="$(mktemp -d)"
cleanup() {
  [[ -f "$work/pgbouncer.pid" ]] && kill "$(cat "$work/pgbouncer.pid")" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

psql -X -q -d postgres -c "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $db" \
     -c "CREATE DATABASE $db" >/dev/null
psql -X -q -d "$db" -c "CREATE EXTENSION jsonb_delta" -c "CREATE EXTENSION pg_tviews" >/dev/null

echo "\"$PGUSER\" \"\"" >"$work/userlist.txt"
cat >"$work/pgbouncer.ini" <<INI
[databases]
$db = host=$PGHOST port=$PGPORT dbname=$db

[pgbouncer]
listen_addr = localhost
listen_port = $port
unix_socket_dir =
auth_type = trust
auth_file = $work/userlist.txt
pool_mode = transaction
max_client_conn = 100
default_pool_size = 20
pidfile = $work/pgbouncer.pid
logfile = $work/pgbouncer.log
INI
"$bouncer" -d "$work/pgbouncer.ini"
for _ in $(seq 50); do
  psql -X -h localhost -p "$port" -d "$db" -tAc "SELECT 1" >/dev/null 2>&1 && break
  sleep 0.2
done

status=0
PGHOST=localhost PGPORT="$port" PGDATABASE="$db" "$here/test-2pc.sh" || status=$?
[[ "$status" -eq 0 ]] || tail -20 "$work/pgbouncer.log"
psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" >/dev/null 2>&1 || true
exit "$status"
