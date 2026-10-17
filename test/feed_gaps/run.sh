#!/usr/bin/env bash
# The change feed's gap counter (#247) against a throwaway cluster whose NOTIFY
# queue fills in seconds: pg_tviews preloaded, max_notify_queue_pages at its
# minimum (PostgreSQL 17+; skipped on older servers). pg_tviews and jsonb_delta
# must be installed for $PGBIN.
#
#   PGBIN=/usr/lib/postgresql/18/bin test/feed_gaps/run.sh
#
# A listener idle in a transaction holds the queue; notifications fill it past
# 90 %; then every write commits unpublished, and another session sees
# pg_tviews_feed_gaps() move by exactly the transactions skipped.
#
# FEED_GAPS_PORT picks the port (default 5499).

set -euo pipefail
PGBIN="${PGBIN:?set PGBIN to the PostgreSQL bin directory}"
PORT="${FEED_GAPS_PORT:-5499}"
dir="$(mktemp -d)"
listener=""
cleanup() {
    [[ -n "$listener" ]] && kill "$listener" 2>/dev/null || true
    "$PGBIN/pg_ctl" -D "$dir/data" -m immediate stop >/dev/null 2>&1 || true
    rm -rf "$dir"
}
trap cleanup EXIT

major="$("$PGBIN/pg_config" --version | sed -E 's/^PostgreSQL ([0-9]+).*/\1/')"
if (( major < 17 )); then
    echo "feed gaps: SKIP (max_notify_queue_pages needs PostgreSQL 17+, found $major)"
    exit 0
fi

"$PGBIN/initdb" -D "$dir/data" -U postgres -A trust >/dev/null
"$PGBIN/pg_ctl" -D "$dir/data" \
    -o "-p $PORT -k $dir -c listen_addresses='' -c shared_preload_libraries=pg_tviews -c max_notify_queue_pages=64" \
    -l "$dir/server.log" -w start >/dev/null

sql() { "$PGBIN/psql" -X -q -At -h "$dir" -p "$PORT" -U postgres -d postgres -v ON_ERROR_STOP=1 "$@"; }
fail() { echo "feed gaps FAIL: $*"; exit 1; }

sql <<'SQL'
SET client_min_messages TO WARNING;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text NOT NULL);
SELECT tviews.pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, t.id, jsonb_build_object('name', t.name) AS data FROM tb_tag t $$);
SQL

# The listener: LISTEN, then stay idle in a transaction, reading nothing.
mkfifo "$dir/listener"
"$PGBIN/psql" -X -q -h "$dir" -p "$PORT" -U postgres -d postgres < "$dir/listener" >/dev/null 2>&1 &
listener=$!
exec 3>"$dir/listener"
echo "LISTEN tviews_changes; BEGIN; SELECT 1;" >&3
for _ in $(seq 50); do
    [[ "$(sql -c "SELECT count(*) FROM pg_stat_activity WHERE state = 'idle in transaction'")" == 1 ]] && break
    sleep 0.1
done

# Fill the queue past 90 % with large notifications, one transaction each.
for _ in $(seq 200); do
    usage="$(sql -c "SELECT pg_notification_queue_usage() > 0.9")"
    [[ "$usage" == t ]] && break
    sql -c "SELECT pg_notify('filler', repeat('x', 7000)) FROM generate_series(1, 4)"
done
[[ "$(sql -c "SELECT pg_notification_queue_usage() > 0.9")" == t ]] || fail "queue never filled"

before="$(sql -c "SELECT tviews.pg_tviews_feed_gaps()")"
out="$(sql 2>&1 <<'SQL'
SET pg_tviews.publish_changes = rows;
INSERT INTO tb_tag (pk_tag, name) VALUES (1, 'a');
INSERT INTO tb_tag (pk_tag, name) VALUES (2, 'b');
BEGIN;
UPDATE tb_tag SET name = 'c';
COMMIT;
SELECT 'per-session counter ' || (tviews.pg_tviews_queue_stats() ? 'publish_skipped');
SQL
)"
grep -q 'per-session counter false' <<<"$out" || fail "queue stats still count skips per session: $out"
grep -q 'not published' <<<"$out" || fail "no WARNING: $out"
after="$(sql -c "SELECT tviews.pg_tviews_feed_gaps()")"
(( after - before == 3 )) || fail "feed gaps moved by $((after - before)), expected 3"

# A rolled-back transaction publishes nothing and is no gap.
sql -c "BEGIN" -c "SET LOCAL pg_tviews.publish_changes = rows" \
    -c "INSERT INTO tb_tag (pk_tag, name) VALUES (3, 'd')" -c "ROLLBACK" >/dev/null
[[ "$(sql -c "SELECT tviews.pg_tviews_feed_gaps()")" == "$after" ]] || fail "a rollback counted"

# The listener reads again once its transaction ends: no more gaps.
echo "COMMIT;" >&3
for _ in $(seq 100); do
    [[ "$(sql -c "SELECT pg_notification_queue_usage() < 0.9")" == t ]] && break
    sleep 0.1
done
sql -c "SET pg_tviews.publish_changes = rows" -c "INSERT INTO tb_tag (pk_tag, name) VALUES (4, 'e')" >/dev/null
[[ "$(sql -c "SELECT tviews.pg_tviews_feed_gaps()")" == "$after" ]] || fail "a published transaction counted"

echo "feed gaps: PASS"
