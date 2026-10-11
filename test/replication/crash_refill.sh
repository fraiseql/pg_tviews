#!/usr/bin/env bash
# After a crash restart, the rebuild launcher refills the reset UNLOGGED TVIEWs of
# every database with pg_tviews, with no configuration and no write
# (pg_tviews.auto_rebuild_databases defaults to `*`). A database without the
# extension is skipped. Runs a throwaway cluster that preloads pg_tviews
# (pg_tviews and jsonb_delta must be installed for $PGBIN).
#
#   PGBIN=/usr/lib/postgresql/18/bin test/replication/crash_refill.sh
#
# CRASH_REFILL_PORT picks the port (default 5497).

set -euo pipefail
PGBIN="${PGBIN:?set PGBIN to the PostgreSQL bin directory}"
PORT="${CRASH_REFILL_PORT:-5497}"
dir="$(mktemp -d)"
fail() { echo "FAIL: $*"; tail -30 "$dir/server.log" 2>/dev/null || true; exit 1; }
cleanup() {
    "$PGBIN/pg_ctl" -D "$dir/data" -m immediate stop >/dev/null 2>&1 || true
    rm -rf "$dir"
}
trap cleanup EXIT

"$PGBIN/initdb" -D "$dir/data" -U postgres -A trust >/dev/null
cat >> "$dir/data/postgresql.conf" <<CONF
shared_preload_libraries = 'pg_tviews'
port = $PORT
listen_addresses = ''
unix_socket_directories = '$dir'
CONF
start() { "$PGBIN/pg_ctl" -D "$dir/data" -l "$dir/server.log" -w start >/dev/null || fail "start"; }
sql() { "$PGBIN/psql" -X -h "$dir" -p "$PORT" -U postgres -qAt -v ON_ERROR_STOP=1 "$@"; }
start

for db in app reporting; do
    sql -d postgres -c "CREATE DATABASE $db"
    sql -d "$db" <<'SQL'
SET client_min_messages TO WARNING;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_item (pk_item, name) SELECT g, 'i' || g FROM generate_series(1, 3) g;
SELECT tviews.pg_tviews_create('tv_item',
    $$SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item$$,
    '{"logged": false}');
CHECKPOINT;
SQL
done
sql -d postgres -c "CREATE DATABASE plain"

"$PGBIN/pg_ctl" -D "$dir/data" -m immediate stop >/dev/null
start

for db in app reporting; do
    filled=""
    for _ in $(seq 1 60); do
        [[ "$(sql -d "$db" -c 'SELECT count(*) FROM tv_item')" == 3 ]] && { filled=yes; break; }
        sleep 0.5
    done
    [[ -n "$filled" ]] || fail "tv_item of database $db was not refilled after the crash restart"
    [[ "$(sql -d "$db" -c 'SELECT bool_and(NOT needs_rebuild) FROM tviews.pg_tviews_replication_status()')" == t ]] \
        || fail "tv_item of database $db still needs a rebuild"
done
grep -q 'extension not installed in database "plain"' "$dir/server.log" \
    || fail "the database without pg_tviews was not skipped quietly"
grep -q 'ERROR' "$dir/server.log" && fail "the rebuild logged an error"
echo "crash refill: PASS"
