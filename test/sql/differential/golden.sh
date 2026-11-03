#!/usr/bin/env bash
# Golden files of the differential shapes: what the oracle (run.sh) cannot see.
# For every shape of schema.sql, expected/<shape>/ holds
#   plan.json      its stored plan (pg_tview_meta.plan), keys sorted, relation OIDs
#                  and attribute numbers written as names (golden_normalize.py).
# expected/_tables/triggers.txt holds the one set of pg_tviews triggers each table
# the shapes read carries: table, name, function, tgtype, enabled state, sorted
# (topology and firing order); expected/_extension/triggers.txt the extension's
# triggers on its own catalog; and expected/_feed/feed.txt the messages a fixed
# script of writes publishes on tviews_changes (publish_changes = rows), ids and
# PIDs masked.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/sql/differential/golden.sh [--bless]
#
# Without --bless, fails on any difference from expected/ and prints it. With
# --bless, rewrites expected/. Explain every golden diff in the commit message.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER
unset PGOPTIONS

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
expected="$here/expected"
bless=0
[[ "${1:-}" == --bless ]] && bless=1
db="pg_tviews_golden_$$"
work="$(mktemp -d)"
trap 'rm -rf "$work"; psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT

psql -d postgres -qc "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db" >/dev/null || exit 2
psql -d postgres -qc "ALTER DATABASE $db SET search_path = \"\$user\", public, tviews" >/dev/null
if ! psql -d "$db" -X -q -v ON_ERROR_STOP=1 -v xfail= -f "$here/schema.sql" >"$work/load.log" 2>&1; then
  echo "ERROR: could not load the schema:"; grep -m3 ERROR "$work/load.log"; exit 2
fi

q() { psql -d "$db" -X -At -v ON_ERROR_STOP=1 "$@"; }
out="$work/out"
mkdir -p "$out/_tables" "$out/_extension" "$out/_feed"

# Every user relation's name and columns, for golden_normalize.py.
q -c "SELECT jsonb_object_agg(c.oid::text, jsonb_build_object(
          'name', format('%s.%s', n.nspname, c.relname),
          'cols', (SELECT coalesce(jsonb_object_agg(a.attnum::text, a.attname), '{}')
                   FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attnum > 0)))
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
      WHERE n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')" >"$work/rels.json"

trigger_list() { # <filter>
  q -F' | ' -c "SELECT t.tgrelid::regclass, t.tgname, t.tgfoid::regproc, t.tgtype, t.tgenabled
                FROM pg_trigger t
                WHERE NOT t.tgisinternal AND $1
                  AND t.tgfoid IN (SELECT d.objid FROM pg_depend d
                                   WHERE d.classid = 'pg_proc'::regclass
                                     AND d.refobjid = (SELECT oid FROM pg_extension WHERE extname = 'pg_tviews'))
                ORDER BY t.tgrelid::regclass::text, t.tgname"
}

while IFS='|' read -r shape entity; do
  mkdir -p "$out/$shape"
  q -c "SELECT plan FROM tviews.pg_tview_meta WHERE entity = '$entity'" |
    python3 "$here/golden_normalize.py" "$work/rels.json" >"$out/$shape/plan.json"
done < <(q -c "SELECT h.tv::regclass::text, m.entity FROM harness_shape h
               JOIN tviews.pg_tview_meta m ON m.table_oid = h.tv ORDER BY 1")
handlers="'pg_tview_trigger_handler', 'pg_tview_flush_trigger', 'pg_tview_delta_trigger', 'pg_tview_truncate_trigger'"
trigger_list "t.tgfoid::regproc::text IN ($handlers)" >"$out/_tables/triggers.txt"
trigger_list "t.tgfoid::regproc::text NOT IN ($handlers)" >"$out/_extension/triggers.txt"

# The feed of a fixed script: the differential harness's first 40 statements of
# seed 1, each published when its transaction commits.
psql -d "$db" -X -q -v ON_ERROR_STOP=1 >"$work/feed.log" 2>&1 <<'SQL'
SET client_min_messages TO WARNING;
SET pg_tviews.publish_changes = rows;
SET harness.xfail = '';
LISTEN tviews_changes;
SELECT setseed(0.5) \g /dev/null
SELECT s FROM harness_script(40) s \gexec
SQL
grep -E 'Asynchronous notification|ERROR' "$work/feed.log" |
  sed -E 's/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/U/g;
          s/server process with PID [0-9]+/server process/' >"$out/_feed/feed.txt"

if [[ "$bless" == 1 ]]; then
  rm -rf "$expected"
  cp -r "$out" "$expected"
  echo "golden files written to $expected"
  exit 0
fi
if diff -ru "$expected" "$out" >"$work/diff"; then
  echo "PASS  golden files ($(find "$expected" -type f | wc -l) files)"
else
  echo "FAIL  golden files differ (rerun with --bless and explain the diff in the commit):"
  sed -e "s|$out|actual|g" -e "s|$expected|expected|g" "$work/diff"
  exit 1
fi
