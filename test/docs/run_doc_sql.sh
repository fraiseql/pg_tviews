#!/usr/bin/env bash
# Run the ```sql blocks of documentation pages as written, in order, in a fresh
# database with nothing preset (no extension, default search_path): a page whose
# SQL fails when followed fails here.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres test/docs/run_doc_sql.sh docs/getting-started/quickstart.md
#
# Honors PGHOST/PGPORT/PGUSER (defaults: localhost / 28818 / postgres). Needs a
# server that preloads pg_tviews, as the pages tell the reader to set up.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER

[[ $# -gt 0 ]] || { echo "usage: $0 page.md ..."; exit 2; }
db="pg_tviews_docs_$$"
sql="$(mktemp)"
out="$(mktemp)"
trap 'rm -f "$sql" "$out"; psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT

fail=0
for page in "$@"; do
  # Every fenced block whose info string is exactly `sql`.
  awk '/^```sql[[:space:]]*$/ {on=1; next} /^```/ {if (on) print ""; on=0; next} on {print}' \
    "$page" >"$sql"
  psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
  psql -d postgres -qc "CREATE DATABASE $db" >/dev/null || { echo "ERROR: cannot create $db"; exit 2; }
  if psql -X -d "$db" -q -v ON_ERROR_STOP=1 -f "$sql" >"$out" 2>&1; then
    echo "PASS  $page"
  else
    echo "FAIL  $page -> $(grep -E '(ERROR|FATAL):|error:' "$out" | head -1)"
    fail=1
  fi
done
exit "$fail"
