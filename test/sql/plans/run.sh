#!/usr/bin/env bash
# Plan snapshots: the walker (src/lineage/walk/) read through the plan registration
# would store, without creating a TVIEW. Each cases/<name>.sql creates its tables
# and a view `v_case` reading them, and names the entity on its first line
# (`-- entity: <entity>`); expected/<name>.json is
# `tviews.pg_tviews_plan_of('v_case', <entity>)`, normalized as the differential
# golden files are (relation OIDs and attribute numbers written as names).
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/sql/plans/run.sh [--bless] [case ...]
#
# Without --bless, fails on any difference and prints it; with --bless, rewrites
# expected/. Explain every snapshot diff in the commit message.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER
unset PGOPTIONS

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
normalize="$here/../differential/golden_normalize.py"
bless=0
[[ "${1:-}" == --bless ]] && { bless=1; shift; }
db="pg_tviews_plans_$$"
work="$(mktemp -d)"
trap 'rm -rf "$work"; psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT

cases=("$@")
[[ ${#cases[@]} -gt 0 ]] || mapfile -t cases < <(cd "$here/cases" && ls ./*.sql | sed 's|^\./||; s|\.sql$||')

fail=0
for name in "${cases[@]}"; do
  file="$here/cases/$name.sql"
  entity="$(sed -n '1s/^-- entity: *//p' "$file")"
  [[ -n "$entity" ]] || { echo "ERROR  $name: no '-- entity:' first line"; fail=1; continue; }
  psql -d postgres -qc "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $db" \
    -c "CREATE DATABASE $db" >/dev/null || exit 2
  if ! psql -d "$db" -X -q -v ON_ERROR_STOP=1 \
        -c "SET client_min_messages = warning" -c "CREATE EXTENSION pg_tviews CASCADE" \
        -f "$file" >"$work/load.log" 2>&1; then
    echo "ERROR  $name: $(grep -m1 ERROR "$work/load.log")"; fail=1; continue
  fi
  psql -d "$db" -X -At -v ON_ERROR_STOP=1 -c "SELECT jsonb_object_agg(c.oid::text, jsonb_build_object(
          'name', format('%s.%s', n.nspname, c.relname),
          'cols', (SELECT coalesce(jsonb_object_agg(a.attnum::text, a.attname), '{}')
                   FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attnum > 0)))
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
      WHERE n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')" >"$work/rels.json"
  if ! psql -d "$db" -X -At -v ON_ERROR_STOP=1 \
        -c "SELECT tviews.pg_tviews_plan_of('v_case'::regclass, '$entity')" \
        >"$work/plan.raw" 2>"$work/plan.err"; then
    echo "ERROR  $name: $(grep -m1 ERROR "$work/plan.err")"; fail=1; continue
  fi
  python3 "$normalize" "$work/rels.json" <"$work/plan.raw" >"$work/$name.json"
  if [[ $bless == 1 ]]; then
    cp "$work/$name.json" "$here/expected/$name.json"
    echo "BLESS  $name"
  elif diff -u "$here/expected/$name.json" "$work/$name.json" >"$work/diff" 2>&1; then
    echo "PASS   $name"
  else
    echo "FAIL   $name"; cat "$work/diff"; fail=1
  fi
done
exit $fail
