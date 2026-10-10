#!/usr/bin/env bash
# Real benchmark for pg_tviews using the ACTUAL pg_tviews_create API.
#
# Compares three approaches for maintaining a denormalised product catalogue
# (product + category + supplier + inventory + review aggregate, one JSONB row
# per product):
#
#   A. pg_tviews + jsonb_delta   (incremental refresh, surgical JSONB patch)
#   B. pg_tviews + native        (incremental refresh, no jsonb_delta — fallback)
#   C. full REFRESH MATERIALIZED VIEW  (traditional O(n) rebuild)
#
# Only tb_product mutations are timed — the operation pg_tviews refreshes
# incrementally and correctly (verified row-for-row against the backing view at
# every scale). The rich denormalised JSON makes a full matview rebuild
# genuinely expensive, which is the point of the comparison.
#
# Measurement: psql \timing on an autocommit statement, so each number is the
# end-to-end client-observed cost INCLUDING the post-statement refresh flush.
# For arm C the timed statement is the REFRESH that a change forces.
#
# Physical cost: every op group is wrapped in bench.snapshot() calls
# (lib/stats.sql) and each arm's bench.physical lands in
# $RUN_DIR/physical.csv as scenario product_<scale>_<delta|native|matview>.
# Arms A/B run once per TVIEW persistence mode (--modes); arm C (a matview) is
# always logged. With EXPLAIN=1 (default) a separate auto_explain pass captures
# the flush statements of one update_single per arm.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./run.sh --scales "small medium large"
#
# Env: PGHOST/PGPORT/PGUSER (defaults localhost / 28818 / postgres), RUN_DIR,
#      MODES ("unlogged logged"), EXPLAIN (1).

set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "$here/lib/common.sh"

SCALES="small"
SINGLE_ITERS=25          # update_single / read iterations for arms A/B
BATCH_ITERS=5            # update_batch iterations
IUD_ITERS=10             # insert_single / delete_single iterations for arms A/B
C_ITERS=5                # per-op iterations for arm C (each is a full REFRESH)

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scales) SCALES="$2"; shift 2 ;;
    --single-iters) SINGLE_ITERS="$2"; shift 2 ;;
    --c-iters) C_ITERS="$2"; shift 2 ;;
    --modes) MODES="$2"; shift 2 ;;
    --help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

outdir="$RUN_DIR"
mkdir -p "$outdir"
raw="$outdir/raw.tsv"
: > "$raw"   # scale <tab> arm <tab> mode <tab> op <tab> ms

scale_counts() {  # -> N_CAT N_SUP N_PROD N_REV
  case "$1" in
    small)  echo "20 10 1000 5000" ;;
    medium) echo "50 30 10000 50000" ;;
    large)  echo "100 100 100000 500000" ;;
    *) echo "unknown scale: $1" >&2; exit 2 ;;
  esac
}

SELECT_BODY="$(cat "$here/product_select.sql")"

db() { psql -X -q -d postgres -c "$1" >/dev/null 2>&1; }

# --- build the arm-A/B (tview) ops script -------------------------------------
# $1 = n_products, $2 = scale label, $3 = mode
tview_script() {
  local nprod="$1" scale="$2" mode="$3" step lo
  step=$(( nprod / SINGLE_ITERS )); [[ $step -lt 1 ]] && step=1
  {
    echo '\timing on'
    echo '\pset pager off'
    echo 'SET client_min_messages TO WARNING;'
    mode_sql "$mode"
    echo "\\echo '@@ build'"
    printf 'SELECT pg_tviews_create(%s, $RB$\n%s\n$RB$);\n' "'tv_product'" "$SELECT_BODY"
    # correctness gate (unmarked -> not measured); fails the run if divergent
    cat <<'SQL'
SELECT CASE WHEN count(*) = 0 THEN 'RB_OK divergence=0'
            ELSE 'RB_DIVERGENCE ' || count(*) END
FROM tv_product t FULL JOIN tviews.public__tv_product v USING (pk_product)
WHERE t.data IS DISTINCT FROM v.data;
SQL
    phys_step_begin
    # update_single: scattered pks
    for ((i=1; i<=SINGLE_ITERS; i++)); do
      local pk=$(( 1 + ((i*step) % nprod) ))
      echo "\\echo '@@ update_single'"
      echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product = $pk;"
    done
    phys_snap update_single "$SINGLE_ITERS"
    # update_batch: 1% of rows per statement, walking windows
    local win=$(( nprod / 100 )); [[ $win -lt 1 ]] && win=1
    for ((i=1; i<=BATCH_ITERS; i++)); do
      lo=$(( 1 + (i-1)*win )); local hi=$(( lo + win - 1 ))
      echo "\\echo '@@ update_batch'"
      echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product BETWEEN $lo AND $hi;"
    done
    phys_snap update_batch "$BATCH_ITERS"
    # insert_single
    for ((i=1; i<=IUD_ITERS; i++)); do
      echo "\\echo '@@ insert_single'"
      echo "INSERT INTO tb_product (fk_category, fk_supplier, sku, name, base_price, current_price) VALUES (1, 1, 'NEW-$scale-$i', 'New product $i', 100, 90);"
    done
    phys_snap insert_single "$IUD_ITERS"
    # delete_single: remove the rows just inserted (no FK fan-out)
    for ((i=1; i<=IUD_ITERS; i++)); do
      echo "\\echo '@@ delete_single'"
      echo "DELETE FROM tb_product WHERE sku = 'NEW-$scale-$i';"
    done
    phys_snap delete_single "$IUD_ITERS"
    # the physical steps must not have broken correctness either
    echo "SELECT CASE WHEN count(*) = 0 THEN 'RB_OK divergence=0' ELSE 'RB_DIVERGENCE ' || count(*) END"
    echo "FROM tv_product t FULL JOIN tviews.public__tv_product v USING (pk_product) WHERE t.data IS DISTINCT FROM v.data;"
  }
}

# --- build the arm-C (matview) ops script -------------------------------------
matview_script() {
  local nprod="$1" scale="$2" step lo
  step=$(( nprod / C_ITERS )); [[ $step -lt 1 ]] && step=1
  {
    echo '\timing on'
    echo '\pset pager off'
    echo 'SET client_min_messages TO WARNING;'
    echo "\\echo '@@ build'"
    printf 'CREATE MATERIALIZED VIEW mv_product AS\n%s;\n' "$SELECT_BODY"
    echo 'CREATE UNIQUE INDEX rb_mv_pk ON mv_product(pk_product);'
    phys_step_begin
    # op-major so each op type gets its own snapshot window
    for ((i=1; i<=C_ITERS; i++)); do
      local pk=$(( 1 + ((i*step) % nprod) ))
      echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product = $pk;"
      echo "\\echo '@@ update_single'"
      echo "REFRESH MATERIALIZED VIEW mv_product;"
    done
    phys_snap update_single "$C_ITERS"
    local win=$(( nprod / 100 )); [[ $win -lt 1 ]] && win=1
    for ((i=1; i<=C_ITERS; i++)); do
      lo=$(( 1 + (i-1)*win )); local hi=$(( lo + win - 1 ))
      echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product BETWEEN $lo AND $hi;"
      echo "\\echo '@@ update_batch'"
      echo "REFRESH MATERIALIZED VIEW mv_product;"
    done
    phys_snap update_batch "$C_ITERS"
    for ((i=1; i<=C_ITERS; i++)); do
      echo "INSERT INTO tb_product (fk_category, fk_supplier, sku, name, base_price, current_price) VALUES (1, 1, 'NEW-$scale-$i', 'New product $i', 100, 90);"
      echo "\\echo '@@ insert_single'"
      echo "REFRESH MATERIALIZED VIEW mv_product;"
    done
    phys_snap insert_single "$C_ITERS"
    for ((i=1; i<=C_ITERS; i++)); do
      echo "DELETE FROM tb_product WHERE sku = 'NEW-$scale-$i';"
      echo "\\echo '@@ delete_single'"
      echo "REFRESH MATERIALIZED VIEW mv_product;"
    done
    phys_snap delete_single "$C_ITERS"
  }
}

# --- parse a \timing log: pair '@@ op' markers with the next 'Time: X ms' ------
parse_log() {  # $1=logfile $2=scale $3=arm $4=mode
  awk -v scale="$2" -v arm="$3" -v mode="$4" '
    /@@ / { split($0, a, "@@ "); op=a[2]; gsub(/[ \t\r]+$/, "", op); pend=op; next }
    /^Time: / && pend != "" { printf "%s\t%s\t%s\t%s\t%s\n", scale, arm, mode, pend, $2; pend="" }
  ' "$1" >> "$raw"
}

arm_label() { case "$1" in a) echo delta ;; b) echo native ;; c) echo matview ;; esac; }

run_arm() {  # $1=scale $2=arm(a|b|c) $3=mode $4=scriptfile
  local scale="$1" arm="$2" mode="$3" script="$4"
  local dbn="bench_rb_$arm"
  local scenario; scenario="product_${scale}_$(arm_label "$arm")"
  db "DROP DATABASE IF EXISTS $dbn"
  db "CREATE DATABASE $dbn TEMPLATE bench_rb_data"
  # The extension lives in schema tviews; the ops script calls it unqualified.
  db "ALTER DATABASE $dbn SET search_path = \"\$user\", public, tviews"
  case "$arm" in
    a) bench_install "$dbn" "jsonb_delta pg_tviews" ;;
    b) bench_install "$dbn" "pg_tviews" ;;
    c) bench_install "$dbn" "" ;;
  esac
  local log="$outdir/${scale}_${arm}_${mode}.log"
  if ! $PSQL -d "$dbn" -f "$script" >"$log" 2>&1; then
    echo "  ARM $arm FAILED (see $log):"; tail -5 "$log"; return 1
  fi
  if grep -q RB_DIVERGENCE "$log"; then
    echo "  ARM $arm CORRECTNESS FAIL:"; grep RB_DIVERGENCE "$log"; return 1
  fi
  parse_log "$log" "$scale" "$arm" "$mode"
  phys_dump "$dbn" "$scenario" "$mode" || return 1
  if [[ "$arm" != c ]]; then
    local ex; ex="$(mktemp)"
    echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product = 1;" >"$ex"
    explain_pass "$dbn" "$ex" "${scenario}_${mode}" 'tv_product'
    rm -f "$ex"
  fi
  db "DROP DATABASE IF EXISTS $dbn"
  echo "  arm $arm ($mode) done"
}

record_env
for scale in $SCALES; do
  read -r NCAT NSUP NPROD NREV <<<"$(scale_counts "$scale")"
  echo "=== scale=$scale (categories=$NCAT suppliers=$NSUP products=$NPROD reviews=$NREV) ==="
  db "DROP DATABASE IF EXISTS bench_rb_data"
  db "CREATE DATABASE bench_rb_data"
  echo "  loading schema + data..."
  $PSQL -d bench_rb_data -f "$here/schema.sql" >/dev/null
  $PSQL -d bench_rb_data -v n_categories="$NCAT" -v n_suppliers="$NSUP" \
        -v n_products="$NPROD" -v n_reviews="$NREV" -f "$here/gen_data.sql" >/dev/null

  ta="$(mktemp)"; tc="$(mktemp)"
  for mode in $MODES; do
    tview_script "$NPROD" "$scale" "$mode" > "$ta"
    run_arm "$scale" a "$mode" "$ta" || exit 1
    run_arm "$scale" b "$mode" "$ta" || exit 1
  done
  matview_script "$NPROD" "$scale" > "$tc"
  run_arm "$scale" c logged "$tc" || exit 1
  rm -f "$ta" "$tc"
  db "DROP DATABASE IF EXISTS bench_rb_data"
done

echo
echo "=== aggregating ($raw) ==="
python3 "$here/aggregate.py" timing "$raw" "$outdir/summary.tsv"
python3 "$here/aggregate.py" physical "$RUN_DIR" -o "$RUN_DIR/report.md"
