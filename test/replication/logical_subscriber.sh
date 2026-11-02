#!/usr/bin/env bash
# TVIEWs over tables a logical replication subscription writes. The apply worker
# runs under session_replication_role = replica (triggers enabled for the origin,
# the default, do not fire) and fires row triggers only, never statement
# triggers: with pg_tviews' triggers enabled ALWAYS its commits fail on the
# unflushed queue and the subscription retries forever; enabled for the origin,
# the TVIEW goes silently stale. Neither may happen: creating a TVIEW over a
# subscribed table fails (0A000, naming the table), and the health check reports
# a table subscribed after its TVIEW was created. A TVIEW on the publisher, its
# table published, replicates like any table.
#
# Runs a throwaway cluster (wal_level = logical, pg_tviews preloaded) with a
# publishing database without pg_tviews and a subscribing one with it
# (pg_tviews and jsonb_delta must be installed for $PGBIN).
#
#   PGBIN=/usr/lib/postgresql/18/bin test/replication/logical_subscriber.sh
#
# LOGICAL_PORT picks the port (default 5496).

set -euo pipefail
PGBIN="${PGBIN:?set PGBIN to the PostgreSQL bin directory}"
PORT="${LOGICAL_PORT:-5496}"
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
wal_level = logical
port = $PORT
listen_addresses = ''
unix_socket_directories = '$dir'
CONF
"$PGBIN/pg_ctl" -D "$dir/data" -l "$dir/server.log" -w start >/dev/null || fail "start"
sql() { "$PGBIN/psql" -X -h "$dir" -p "$PORT" -U postgres -qAt -v ON_ERROR_STOP=1 "$@"; }

schema="CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL, name text);
        CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL,
                              fk_user int NOT NULL REFERENCES tb_user, title text);"
sql -d postgres -c "CREATE DATABASE pub" -c "CREATE DATABASE sub"
sql -d pub -c "$schema" -c "INSERT INTO tb_user VALUES (1, gen_random_uuid(), 'ada')" \
    -c "INSERT INTO tb_post VALUES (1, gen_random_uuid(), 1, 'p1')" \
    -c "CREATE PUBLICATION tviews_pub FOR TABLE tb_user, tb_post" \
    -c "SELECT pg_create_logical_replication_slot('tviews_sub', 'pgoutput')" >/dev/null
sql -d sub -c "$schema" -c "SET client_min_messages = warning" \
    -c "CREATE EXTENSION jsonb_delta" -c "CREATE EXTENSION pg_tviews" \
    -c "CREATE SUBSCRIPTION tviews_sub CONNECTION 'host=$dir port=$PORT dbname=pub user=postgres'
        PUBLICATION tviews_pub WITH (create_slot = false, slot_name = 'tviews_sub')" >/dev/null

# The initial copy.
for _ in $(seq 100); do
    [[ "$(sql -d sub -c "SELECT count(*) FROM pg_subscription_rel WHERE srsubstate <> 'r'")" == 0 ]] && break
    sleep 0.1
done
[[ "$(sql -d sub -c "SELECT count(*) FROM tb_post")" == 1 ]] || fail "the initial copy did not arrive"

if out="$(sql -d sub 2>&1 <<'SQL'
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
SQL
)"; then
    fail "a TVIEW over subscribed tables was created"
fi
grep -q 'logical replication subscription writes' <<<"$out" \
    || fail "the refusal does not say why: $out"

# A TVIEW created before its tables were subscribed: the health check reports it.
sql -d sub -c "CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL, body text)" \
    -c "SELECT tviews.pg_tviews_create('tv_note', 'SELECT pk_note, id, jsonb_build_object(''body'', body) AS data FROM tb_note')" >/dev/null
sql -d pub -c "CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL, body text)" \
    -c "ALTER PUBLICATION tviews_pub ADD TABLE tb_note"
sql -d sub -c "ALTER SUBSCRIPTION tviews_sub REFRESH PUBLICATION" >/dev/null
health="$(sql -d sub -c "SELECT status || ' ' || message FROM tviews.pg_tviews_health_check()
                         WHERE component = 'triggers'")"
grep -q '^ERROR' <<<"$health" && grep -q 'tb_note' <<<"$health" \
    && grep -q 'subscription' <<<"$health" \
    || fail "the health check does not report a TVIEW over a subscribed table: $health"

sql -d sub -c "DROP SUBSCRIPTION tviews_sub" >/dev/null
echo "logical subscriber: PASS"
