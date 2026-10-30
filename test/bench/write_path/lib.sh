# shellcheck shell=bash
# Shared by run.sh, fanout.sh and compare.sh. Source it; needs PGHOST/PGPORT/PGUSER.

# measure <db> <pgbench file> <seconds> <clients> [PGOPTIONS]: run pgbench on <db> and
# print "<tps> <ms per transaction> <WAL bytes per transaction>". The WAL is the
# difference of pg_current_wal_lsn() around the run, divided by the transactions.
measure() {
  local db="$1" file="$2" secs="$3" clients="$4" opts="${5:-}" l0 l1 out n
  psql -X -q -d "$db" -c "CHECKPOINT" >/dev/null
  l0="$(psql -X -At -d "$db" -c "SELECT pg_current_wal_lsn()")"
  out="$(PGOPTIONS="$opts" pgbench -n -c "$clients" -j "$clients" -T "$secs" -f "$file" "$db" 2>&1)"
  l1="$(psql -X -At -d "$db" -c "SELECT pg_current_wal_lsn()")"
  n="$(sed -n 's/^number of transactions actually processed: \([0-9]*\).*/\1/p' <<<"$out")"
  if [[ -z "$n" || "$n" == 0 ]]; then echo "pgbench failed: $(tail -3 <<<"$out")" >&2; echo "0 0 0"; return; fi
  printf '%s %s %s\n' \
    "$(sed -n 's/^tps = \([0-9.]*\).*/\1/p' <<<"$out" | cut -d. -f1)" \
    "$(sed -n 's/^latency average = \([0-9.]*\) ms/\1/p' <<<"$out")" \
    "$(psql -X -At -d "$db" -c "SELECT round(pg_wal_lsn_diff('$l1', '$l0') / $n)")"
}

# fresh_copy <template> <db>: <db> becomes a new copy of <template>, so that a run
# never measures a table an earlier run bloated. FILE_COPY: no WAL for the copy.
fresh_copy() {
  psql -X -q -d postgres -c "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $2" \
       -c "CREATE DATABASE $2 TEMPLATE $1 STRATEGY FILE_COPY" >/dev/null
}

# seed_template <template> <schema file>: create and seed <template> unless it holds
# the seeded schema already.
seed_template() {
  if ! psql -X -d "$1" -tAc "SELECT 1 FROM bench.tv_comment, bench.tb_audit LIMIT 1" >/dev/null 2>&1; then
    psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $1" -c "CREATE DATABASE $1"
    psql -X -q -v ON_ERROR_STOP=1 -d "$1" -c "SET pg_tviews.max_queue_size = 1000000" -f "$2" >/dev/null
  fi
}
