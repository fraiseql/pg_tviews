# shellcheck shell=bash
# Shared helpers for the physical-cost benchmark (sourced, not executed).
#
# Conventions:
#   RUN_DIR   results/physical/<run id>; every scenario writes into it
#   MODES     "unlogged logged": TVIEW persistence via pg_tviews.unlogged_by_default
#             (unlogged is the shipped default, so it runs first)
#   EXPLAIN   1 (default) runs a separate auto_explain pass per scenario+mode
#
# A scenario builds a psql script, wrapping each measured step in snapshots
# (`phys_step_begin`, then `phys_snap <label> <ops>` after it), runs it, then
# calls `phys_dump` to write that database's bench.physical as CSV.

bench_lib="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bench_root="$(dirname "$bench_lib")"

export PGHOST="${PGHOST:-localhost}" PGPORT="${PGPORT:-28818}" PGUSER="${PGUSER:-postgres}"
MODES="${MODES:-unlogged logged}"
EXPLAIN="${EXPLAIN:-1}"
RUN_DIR="${RUN_DIR:-$bench_root/results/physical/$(date +%Y%m%d-%H%M%S)}"
export RUN_DIR
mkdir -p "$RUN_DIR"

PSQL="psql -X -v ON_ERROR_STOP=1 -q"

db_exec() { psql -X -q -d postgres -c "$1" >/dev/null 2>&1; }

# Recreate database $1, optionally from template $2.
db_fresh() {
  db_exec "DROP DATABASE IF EXISTS $1"
  if [[ -n "${2:-}" ]]; then
    psql -X -q -d postgres -c "CREATE DATABASE $1 TEMPLATE $2" >/dev/null
  else
    psql -X -q -d postgres -c "CREATE DATABASE $1" >/dev/null
  fi
}

# Install extensions $2 (default "jsonb_delta pg_tviews") plus the snapshot
# helpers (and pg_visibility, when available) into db $1.
bench_install() {
  local ext
  for ext in ${2-jsonb_delta pg_tviews}; do
    $PSQL -d "$1" -c "SET client_min_messages TO WARNING; CREATE EXTENSION IF NOT EXISTS $ext" >/dev/null
  done
  $PSQL -d "$1" -f "$bench_lib/stats.sql" >/dev/null
}

# SQL that makes the following pg_tviews_create() honour mode $1.
mode_sql() {
  case "$1" in
    unlogged) echo "SET pg_tviews.unlogged_by_default = on;" ;;
    logged)   echo "SET pg_tviews.unlogged_by_default = off;" ;;
    *) echo "unknown mode: $1" >&2; return 2 ;;
  esac
}

# Zero the counters and quiesce before the first measured step: autovacuum off
# on every user table (it would reset n_dead_tup mid-step), VACUUM (ANALYZE)
# after the reset so n_live_tup is repopulated and the visibility map starts
# clean, CHECKPOINT so full-page-image counts start from a fresh cycle.
phys_step_begin() {
  cat <<'SQL'
SELECT bench.reset();
SELECT format('ALTER TABLE %I.%I SET (autovacuum_enabled = false)', schemaname, relname)
FROM pg_stat_user_tables WHERE schemaname <> 'bench' \gexec
VACUUM (ANALYZE);
CHECKPOINT;
SELECT pg_stat_force_next_flush();
SELECT bench.snapshot('start');
SQL
}

# Close a step: snapshot labelled $1, $2 operations in it (default NULL).
phys_snap() {
  echo "SELECT pg_stat_force_next_flush();"
  echo "SELECT bench.snapshot('$1', ${2:-NULL});"
}

# Fail the run if TVIEW $1 diverges from its backing view $2 on pk column $3.
divergence_gate() {
  cat <<SQL
SELECT CASE WHEN count(*) = 0 THEN 'RB_OK $1'
            ELSE 'RB_DIVERGENCE $1 ' || count(*) END
FROM $1 t FULL JOIN $2 v USING ($3)
WHERE t.data IS DISTINCT FROM v.data;
SQL
}

# Run psql script $2 against db $1, logging to $3; fail on error or divergence.
run_script() {
  if ! $PSQL -d "$1" -f "$2" >"$3" 2>&1; then
    echo "  FAILED (see $3):"; tail -5 "$3"; return 1
  fi
  if grep -q RB_DIVERGENCE "$3"; then
    echo "  CORRECTNESS FAIL:"; grep RB_DIVERGENCE "$3"; return 1
  fi
}

# Append db $1's bench.physical to $RUN_DIR/physical.csv tagged scenario=$2 mode=$3.
phys_dump() {
  local out="$RUN_DIR/physical.csv" header=HEADER
  [[ -s "$out" ]] && header=""
  $PSQL -d "$1" -c "COPY (SELECT '$2' AS scenario, '$3' AS mode, * FROM bench.physical
                          ORDER BY seq, relname) TO STDOUT WITH (FORMAT csv${header:+, HEADER})" >>"$out"
}

# auto_explain pass: run the SQL in $2 against db $1 and keep plans whose query
# text matches $4 under $RUN_DIR/explain/$3/.
explain_pass() {
  [[ "$EXPLAIN" == "1" ]] || return 0
  local log="$RUN_DIR/explain/$3.log"
  mkdir -p "$RUN_DIR/explain"
  { echo "\\i $bench_lib/explain_on.sql"; cat "$2"; } | $PSQL -d "$1" >"$log" 2>&1 \
    || { echo "  explain pass FAILED (see $log)"; return 1; }
  python3 "$bench_lib/explain_extract.py" "$log" "$RUN_DIR/explain/$3" --match "$4" >/dev/null \
    || echo "  explain pass: no plan matched /$4/ (see $log)"
}

# Record machine, server and build metadata once per run.
record_env() {
  local out="$RUN_DIR/env.tsv"
  [[ -s "$out" ]] && return 0
  {
    printf 'date\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'commit\t%s\n' "$(git -C "$bench_root" describe --always --dirty 2>/dev/null)"
    printf 'cpu\t%s\n' "$(lscpu 2>/dev/null | sed -n 's/^Model name:[[:space:]]*//p' | head -1)"
    printf 'cores\t%s\n' "$(nproc)"
    printf 'memory\t%s\n' "$(free -h 2>/dev/null | awk '/^Mem:/{print $2}')"
    printf 'kernel\t%s\n' "$(uname -sr)"
    psql -X -d postgres -tA -F $'\t' -c "
      SELECT 'server_version', current_setting('server_version')
      UNION ALL SELECT name, setting || coalesce(unit, '') FROM pg_settings
      WHERE name IN ('shared_buffers', 'checkpoint_timeout', 'max_wal_size', 'wal_level',
                     'full_page_writes', 'wal_compression', 'synchronous_commit', 'fsync',
                     'autovacuum')
         OR name LIKE 'pg_tviews.%'
      UNION ALL SELECT 'ext_' || name, default_version FROM pg_available_extensions
      WHERE name IN ('pg_tviews', 'jsonb_delta')"
  } >"$out"
}
