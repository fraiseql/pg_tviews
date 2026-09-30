#!/usr/bin/env bash
# Physical-replication contract for UNLOGGED TVIEWs (issue #75), on a real standby.
#
#   1. On a hot standby, an UNLOGGED tv_* cannot be read, a LOGGED one can, and
#      pg_tviews_replication_status() works (read-only).
#   2. After promotion, the pg_tviews.auto_rebuild_databases worker repopulates
#      the UNLOGGED TVIEW without any write.
#   3. After an immediate (crash) stop and restart of the promoted node, the
#      worker repopulates it again.
#
# The standby is a pg_basebackup of the primary on a spare port, in a temporary
# directory; the primary is not restarted. Needs pg_tviews preloaded on the
# primary and a replication-enabled pg_hba.conf.
#
# Usage:
#   PGBIN=/path/to/pg/bin PGHOST=localhost PGPORT=28818 PGUSER=postgres \
#     STANDBY_PORT=28819 ./test/replication/promote_rebuild.sh

set -euo pipefail
PGBIN="${PGBIN:-$(dirname "$(command -v pg_ctl)")}"
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
STANDBY_PORT="${STANDBY_PORT:-28819}"
# Never prompt for a password: a missing trust rule should fail, not hang.
export PGHOST PGPORT PGUSER PGCONNECT_TIMEOUT=10

db="pg_tviews_repl_$$"
standby="$(mktemp -d)/standby"
fail() { echo "FAIL: $*"; exit 1; }
cleanup() {
  "$PGBIN/pg_ctl" -D "$standby" -m immediate stop >/dev/null 2>&1 || true
  rm -rf "$(dirname "$standby")"
  "$PGBIN/psql" -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1 || true
}
trap cleanup EXIT

primary() { "$PGBIN/psql" -w -d "$db" -qAtX -v ON_ERROR_STOP=1 "$@"; }
node() { "$PGBIN/psql" -w -h localhost -p "$STANDBY_PORT" -d "$db" -qAtX "$@" 2>&1 || true; }
start_node() {
  "$PGBIN/pg_ctl" -D "$standby" -l "$standby/log" -w start >/dev/null \
    || { tail -20 "$standby/log"; fail "standby did not start"; }
}
wait_for_rows() { # <expected> <what>
  for _ in $(seq 1 60); do
    [[ "$(node -c "SELECT count(*) FROM tv_post")" == "$1" ]] && return 0
    sleep 0.5
  done
  fail "$2: tv_post has $(node -c "SELECT count(*) FROM tv_post") rows, expected $1"
}

"$PGBIN/psql" -w -d postgres -qc "CREATE DATABASE $db"
"$PGBIN/psql" -w -d postgres -qc "ALTER DATABASE $db SET search_path = \"\$user\", public, tviews"
primary >/dev/null 2>&1 <<'SQL'
SET client_min_messages TO WARNING;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_post (pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, title TEXT);
CREATE TABLE tb_tag (pk_tag BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                     id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, label TEXT);
INSERT INTO tb_post (title) VALUES ('a'), ('b');
INSERT INTO tb_tag (label) VALUES ('x');
CREATE TABLE tv_post AS SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;
BEGIN;
SET LOCAL pg_tviews.unlogged_by_default = off;
CREATE TABLE tv_tag AS SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM tb_tag;
COMMIT;
CHECKPOINT;
SQL

"$PGBIN/pg_basebackup" -w -c fast -p "$PGPORT" -D "$standby" -R -X stream
# Packaged clusters (Debian/Ubuntu) keep their config outside the data directory.
[[ -f "$standby/postgresql.conf" ]] || : > "$standby/postgresql.conf"
[[ -f "$standby/pg_hba.conf" ]] || printf 'local all all trust\nhost all all 127.0.0.1/32 trust\nhost all all ::1/128 trust\n' > "$standby/pg_hba.conf"
{
  echo "port = $STANDBY_PORT"
  echo "listen_addresses = 'localhost'"
  echo "unix_socket_directories = ''"
  echo "hba_file = '$standby/pg_hba.conf'"
  echo "pg_tviews.auto_rebuild_databases = '$db'"
} >> "$standby/postgresql.auto.conf"
start_node

# 1. Hot standby
[[ "$(node -c "SELECT pg_is_in_recovery()")" == "t" ]] || fail "standby is not in recovery"
out="$(node -c "SELECT count(*) FROM tv_post" || true)"
[[ "$out" == *"cannot access temporary or unlogged relations during recovery"* ]] \
  || fail "reading the UNLOGGED tv_post on the standby: $out"
[[ "$(node -c "SELECT count(*) FROM tv_tag")" == "1" ]] || fail "LOGGED tv_tag not readable on the standby"
status="$(node -c "SELECT string_agg(concat_ws(':', entity, persistence, replica_readable, coalesce(is_empty::text, 'null'), coalesce(needs_rebuild::text, 'null')), ',' ORDER BY entity) FROM pg_tviews_replication_status()")"
[[ "$status" == "post:unlogged:f:null:null,tag:logged:t:false:null" ]] \
  || fail "replication_status on the standby: $status"
profile="$(node -c "SELECT string_agg(entity || ':' || persistence, ',' ORDER BY entity) FROM pg_tviews_profile()")"
[[ "$profile" == "post:unlogged,tag:logged" ]] || fail "pg_tviews_profile() on the standby: $profile"
echo "PASS  standby: UNLOGGED unreadable, LOGGED readable, status and profile callable"

# 2. Promotion
"$PGBIN/pg_ctl" -D "$standby" -w promote >/dev/null
wait_for_rows 2 "after promotion"
echo "PASS  promotion: tv_post rebuilt by the worker without any write"

# 3. Crash restart of the promoted node
"$PGBIN/pg_ctl" -D "$standby" -m immediate stop >/dev/null
start_node
wait_for_rows 2 "after crash restart"
echo "PASS  crash restart: tv_post rebuilt by the worker without any write"
