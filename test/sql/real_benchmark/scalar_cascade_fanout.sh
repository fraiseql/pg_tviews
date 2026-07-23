#!/usr/bin/env bash
# Issue #56 benchmark: single-field update with a wide nested-parent fan-out.
#
# Models the issue's motivating shape — 1 author, N posts each embedding the
# author as a nested object (tv_post.data->'author') — and times
# `UPDATE tb_user SET bio = 'bio-'||i` (each its own statement) with the
# direct-patch fast path OFF (today's recompute baseline) vs ON.
#
# The timed loop runs SERVER-SIDE (a plpgsql loop, one statement per iteration so
# the per-statement flush trigger still fires) to isolate the refresh cost from
# client round-trip overhead. Counters are read in the SAME session (they are
# per-backend thread-local) to prove the fast path did zero backing-view
# recomputes. Absolute throughput is machine-specific; the RATIO is the portable
# result.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./scalar_cascade_fanout.sh [FANOUT] [ITERS]

set -u
export PGHOST="${PGHOST:-localhost}" PGPORT="${PGPORT:-28818}" PGUSER="${PGUSER:-postgres}"
FANOUT="${1:-60}"      # posts per author
ITERS="${2:-2000}"     # timed single-field updates per configuration
DB="pg_tviews_bench_56_$$"
PSQL="psql -X -v ON_ERROR_STOP=1 -q -t -A"

cleanup() { psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $DB" >/dev/null 2>&1; }
trap cleanup EXIT

psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $DB" >/dev/null 2>&1
psql -X -q -d postgres -c "CREATE DATABASE $DB" >/dev/null 2>&1

psql -X -q -v ON_ERROR_STOP=1 -d "$DB" >/dev/null <<SQL
SET client_min_messages TO ERROR;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_user (pk_user INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, name TEXT, bio TEXT);
CREATE TABLE tb_post (pk_post INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
                      fk_user INT REFERENCES tb_user(pk_user), title TEXT);
INSERT INTO tb_user (name, bio) VALUES ('Author', 'bio-0');
INSERT INTO tb_post (fk_user, title)
     SELECT 1, 'post-'||g FROM generate_series(1, $FANOUT) g;
SELECT pg_tviews_create('tv_user', \$t\$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user \$t\$);
SELECT pg_tviews_create('tv_post', \$t\$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object('title', tb_post.title, 'author', v_user.data) AS data
    FROM tb_post LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user \$t\$);
SQL

run_config() {  # $1 = on|off  -> "writes_per_s recomputes patches_applied"
  local guc="$1"
  $PSQL -d "$DB" <<SQL | grep '^RESULT' | tail -1 | sed 's/^RESULT|//' | tr '|' ' '
SET client_min_messages TO ERROR;
SET pg_tviews.direct_patch_enabled = $guc;
UPDATE tb_user SET bio = 'warm' WHERE pk_user = 1;
SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint AS rc0,
       (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint AS ap0 \gset
SELECT extract(epoch FROM clock_timestamp()) AS t0 \gset
DO \$b\$ BEGIN
  FOR i IN 1..$ITERS LOOP
    UPDATE tb_user SET bio = 'bio-'||i WHERE pk_user = 1;
  END LOOP;
END \$b\$;
SELECT extract(epoch FROM clock_timestamp()) AS t1 \gset
SELECT 'RESULT|' || round($ITERS / (:t1 - :t0))::text
       || '|' || ((pg_tviews_queue_stats()->>'view_recomputes')::bigint - :rc0)::text
       || '|' || ((pg_tviews_queue_stats()->>'direct_patches_applied')::bigint - :ap0)::text;
SQL
}

echo "== issue #56 scalar_cascade_fanout =="
echo "server: $(psql -X -tAc 'SHOW server_version' | tr -d ' ')  fan-out: $FANOUT posts  iters: $ITERS"
read -r off_wps off_rc off_ap < <(run_config off)
read -r on_wps  on_rc  on_ap  < <(run_config on)
ratio=$(awk "BEGIN{printf \"%.1f\", $on_wps/$off_wps}")

printf "%-20s %12s %14s %16s\n" "config" "writes/s" "recomputes" "patches_applied"
printf "%-20s %12s %14s %16s\n" "GUC off (baseline)" "$off_wps" "$off_rc" "$off_ap"
printf "%-20s %12s %14s %16s\n" "GUC on (fast path)" "$on_wps" "$on_rc" "$on_ap"
echo "speedup (on/off): ${ratio}x   (each update fans out to $FANOUT posts)"
