#!/usr/bin/env bash
# Isolation specs (test/isolation/specs/*.spec): concurrent writers, DDL and
# snapshot isolation levels, run by pg_isolation_regress against an existing
# cluster with pg_tviews and jsonb_delta installed and pg_tviews preloaded.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/isolation/run.sh [spec ...]
#
# PG_CONFIG names the server's pg_config (default: the one on PATH). The output
# of each spec is compared with expected/<spec>.out; a difference fails the run
# and is printed.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
PG_CONFIG="${PG_CONFIG:-pg_config}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
regress="$("$PG_CONFIG" --pkglibdir)/pgxs/src/test/isolation/pg_isolation_regress"
[[ -x "$regress" ]] || { echo "ERROR: $regress not found (install the server dev package)"; exit 2; }
db=pg_tviews_isolation_$$
psql -d postgres -qc "CREATE DATABASE $db" || exit 2
trap 'psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT
out="$(mktemp -d)"
if [[ $# -gt 0 ]]; then specs=("$@"); else specs=(--schedule="$here/isolation_schedule"); fi
"$regress" --use-existing --host="$PGHOST" --port="$PGPORT" --user="$PGUSER" \
  --bindir="$("$PG_CONFIG" --bindir)" --inputdir="$here" --outputdir="$out" \
  --dbname="$db" "${specs[@]}"
rc=$?
[[ $rc -ne 0 && -f "$out/regression.diffs" ]] && cat "$out/regression.diffs"
[[ $rc -eq 0 ]] && rm -rf "$out" || echo "results kept in $out"
exit $rc
