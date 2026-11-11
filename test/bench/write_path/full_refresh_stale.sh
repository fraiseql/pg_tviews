#!/usr/bin/env bash
# How long a full refresh takes when the TVIEW table's statistics are stale
# (#301). A TVIEW of N rows reads a one-row table, tb_flag, in an uncorrelated
# subquery (all_keys under the full_refresh policy): a write to tb_flag brings the
# whole TVIEW to its view. tb_flag.min_v filters the rows (a raise deletes, a
# lowering inserts) and tb_flag.label is in every row (a change updates them all).
# Two shapes: "plain" reads tb_item alone; "aggregate" also sums five tb_line rows
# per item in a GROUP BY subquery, a view that cannot be probed by key.
#
# Each shape and N runs twice on a fresh database: "fresh" with the TVIEW table
# analyzed at N rows, "stale" with it analyzed while empty and refilled since. Each
# step prints its wall time (or "timeout") and, for each statement writing the
# TVIEW, its time and first join node (auto_explain, nested statements, sent to the
# client); the plans are kept in $OUT (default: a temp dir).
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/full_refresh_stale.sh
#   SIZES="20000 200000"     the TVIEW sizes (default)
#   SHAPES="plain aggregate" the view shapes (default)
#   TIMEOUT=120s             a step's statement_timeout (default)
#   OUT=dir                  keep each run's plans in dir
set -euo pipefail
read -r -a sizes <<<"${SIZES:-20000 200000}"
read -r -a shapes <<<"${SHAPES:-plain aggregate}"
timeout="${TIMEOUT:-120s}"
db=pg_tviews_full_refresh_stale_bench
out="${OUT:-$(mktemp -d)}"
mkdir -p "$out"
trap 'psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT

# step <file> <label> <sql>: run <sql> in its own transaction with auto_explain on,
# print "<label> <ms, all its statements>" and, for each statement that writes the
# TVIEW, its kind, time and first join node, and append its plans to <file>.
step() {
  local file="$1" label="$2" sql="$3" log ms joins
  log="$(psql -X -q -v ON_ERROR_STOP=1 -d "$db" 2>&1 <<SQL || true
SET client_min_messages = log;
SET statement_timeout = '$timeout';
LOAD 'auto_explain';
SET auto_explain.log_min_duration = 0;
SET auto_explain.log_analyze = on;
SET auto_explain.log_nested_statements = on;
SET auto_explain.log_level = log;
SET pg_tviews.max_queue_size = 10000000;
\timing on
$sql;
SQL
)"
  {
    echo "=== $label"
    grep -v '^Time:' <<<"$log"
  } >>"$file"
  ms="$(sed -n 's/^Time: \([0-9.]*\) ms.*/\1/p' <<<"$log" | awk '{ t += $1 } END { printf "%.1f", t }')"
  if grep -q 'canceling statement due to statement timeout' <<<"$log"; then ms="timeout"; fi
  # Each statement that writes tv_item: its kind, its time and its first join node.
  joins="$(awk '
    /^LOG:  duration: / { ms = $3; q = ""; next }
    /^Query Text: / {
      q = ""
      if ($0 ~ /^Query Text: DELETE FROM public\.tv_item/) q = "delete"
      else if ($0 ~ /^Query Text: UPDATE public\.tv_item/) q = "update"
      else if ($0 ~ /^Query Text: INSERT INTO public\.tv_item/) q = "insert"
      if (q != "") { printf "%s%s %sms", sep, q, ms; sep = "; "; want = 1 }
      next
    }
    want && match($0, /(Nested Loop|Hash|Merge)( (Anti|Semi|Right Anti|Left))? Join/) {
      printf " %s", substr($0, RSTART, RLENGTH); want = 0
    }' <<<"$log")"
  printf '%-9s %-8s %-6s %-26s %10s  %s\n' "$shape" "$n" "$mode" "$label" "$ms" "$joins"
}

# definition: the TVIEW's definition for $shape.
definition() {
  local label="(SELECT f.label FROM tb_flag f WHERE f.pk_flag = 1)"
  local min_v="(SELECT f.min_v FROM tb_flag f WHERE f.pk_flag = 1)"
  case "$shape" in
    plain) echo "SELECT i.pk_item, i.id,
               jsonb_build_object('name', i.name, 'v', i.v, 'label', $label) AS data
             FROM tb_item i WHERE i.v >= $min_v" ;;
    aggregate) echo "SELECT i.pk_item, i.id,
               jsonb_build_object('name', i.name, 'v', i.v, 'total', s.total, 'label', $label) AS data
             FROM tb_item i
             JOIN (SELECT l.fk_item, sum(l.n) AS total FROM tb_line l GROUP BY l.fk_item) s
               ON s.fk_item = i.pk_item
             WHERE i.v >= $min_v" ;;
    *) echo "unknown shape $shape" >&2; exit 2 ;;
  esac
}

seed() {
  psql -X -q -d postgres -c "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $db" \
       -c "CREATE DATABASE $db" >/dev/null
  psql -X -q -v ON_ERROR_STOP=1 -d "$db" >/dev/null <<SQL
SET client_min_messages = warning;
SET pg_tviews.max_queue_size = 10000000;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_flag (pk_flag int PRIMARY KEY, min_v int NOT NULL, label text NOT NULL);
INSERT INTO tb_flag VALUES (1, 0, 'a');
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      v int NOT NULL, name text NOT NULL);
INSERT INTO tb_item (pk_item, v, name) SELECT g, g % 100, 'item ' || g FROM generate_series(1, $n) g;
CREATE TABLE tb_line (pk_line int PRIMARY KEY, fk_item int NOT NULL REFERENCES tb_item, n int NOT NULL);
INSERT INTO tb_line (pk_line, fk_item, n) SELECT g, 1 + g % $n, g % 7 FROM generate_series(1, 5 * $n) g;
CREATE INDEX ON tb_line (fk_item);
SELECT tviews.pg_tviews_create('tv_item', \$q\$$(definition)\$q\$,
    options => '{"uncascaded_policy": "full_refresh"}');
ALTER TABLE tv_item SET (autovacuum_enabled = off);
VACUUM ANALYZE;
SQL
}

printf '%-9s %-8s %-6s %-26s %10s  %s\n' shape rows stats step ms joins
for shape in "${shapes[@]}"; do
for n in "${sizes[@]}"; do
  for mode in fresh stale; do
    seed
    file="$out/plans-$shape-$n-$mode.txt"
    : >"$file"
    if [[ "$mode" == stale ]]; then
      # Analyzed while empty, refilled since: the statistics say 0 rows.
      step "$file" "empty (min_v = 100)" "UPDATE tb_flag SET min_v = 100"
      psql -X -q -d "$db" -c "ANALYZE tv_item" >/dev/null
      step "$file" "refill (min_v = 0)" "UPDATE tb_flag SET min_v = 0"
    fi
    psql -X -At -d "$db" -c "SELECT 'tv_item: ' || reltuples::bigint || ' rows in the statistics, ' \
         || (SELECT count(*) FROM tv_item) || ' rows' FROM pg_class WHERE oid = 'tv_item'::regclass" >>"$file"
    step "$file" "update all (label)" "UPDATE tb_flag SET label = 'b'"
    step "$file" "delete half (min_v = 50)" "UPDATE tb_flag SET min_v = 50"
    step "$file" "insert half (min_v = 0)" "UPDATE tb_flag SET min_v = 0"
    step "$file" "catch-up after resume" "BEGIN;
SELECT tviews.pg_tviews_suspend_triggers();
UPDATE tb_flag SET label = 'c';
SELECT tviews.pg_tviews_resume_triggers();
COMMIT"
  done
done
done
echo "plans: $out"
