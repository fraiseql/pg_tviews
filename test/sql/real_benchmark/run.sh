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
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./run.sh --scales "small medium large"
#
# Env: PGHOST/PGPORT/PGUSER (defaults localhost / 28818 / postgres).

set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PGHOST="${PGHOST:-localhost}" PGPORT="${PGPORT:-28818}" PGUSER="${PGUSER:-postgres}"

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
    --help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

outdir="$here/results"
mkdir -p "$outdir"
raw="$outdir/raw.tsv"
: > "$raw"   # scale <tab> arm <tab> op <tab> ms

PSQL="psql -X -v ON_ERROR_STOP=1 -q"

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
# $1 = n_products, $2 = scale label
tview_script() {
  local nprod="$1" scale="$2" step lo
  step=$(( nprod / SINGLE_ITERS )); [[ $step -lt 1 ]] && step=1
  {
    echo '\timing on'
    echo '\pset pager off'
    echo 'SET client_min_messages TO WARNING;'
    echo "\\echo '@@ build'"
    printf 'SELECT pg_tviews_create(%s, $RB$\n%s\n$RB$);\n' "'tv_product'" "$SELECT_BODY"
    # correctness gate (unmarked -> not measured); fails the run if divergent
    cat <<'SQL'
SELECT CASE WHEN count(*) = 0 THEN 'RB_OK divergence=0'
            ELSE 'RB_DIVERGENCE ' || count(*) END
FROM tv_product t FULL JOIN v_product v USING (pk_product)
WHERE t.data IS DISTINCT FROM v.data;
SQL
    # update_single: scattered pks
    for ((i=1; i<=SINGLE_ITERS; i++)); do
      local pk=$(( 1 + ((i*step) % nprod) ))
      echo "\\echo '@@ update_single'"
      echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product = $pk;"
    done
    # update_batch: 1% of rows per statement, walking windows
    local win=$(( nprod / 100 )); [[ $win -lt 1 ]] && win=1
    for ((i=1; i<=BATCH_ITERS; i++)); do
      lo=$(( 1 + (i-1)*win )); local hi=$(( lo + win - 1 ))
      echo "\\echo '@@ update_batch'"
      echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product BETWEEN $lo AND $hi;"
    done
    # insert_single
    for ((i=1; i<=IUD_ITERS; i++)); do
      echo "\\echo '@@ insert_single'"
      echo "INSERT INTO tb_product (fk_category, fk_supplier, sku, name, base_price, current_price) VALUES (1, 1, 'NEW-$scale-$i', 'New product $i', 100, 90);"
    done
    # delete_single: remove the rows just inserted (no FK fan-out)
    for ((i=1; i<=IUD_ITERS; i++)); do
      echo "\\echo '@@ delete_single'"
      echo "DELETE FROM tb_product WHERE sku = 'NEW-$scale-$i';"
    done
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
    local win=$(( nprod / 100 )); [[ $win -lt 1 ]] && win=1
    for ((i=1; i<=C_ITERS; i++)); do
      local pk=$(( 1 + ((i*step) % nprod) ))
      echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product = $pk;"
      echo "\\echo '@@ update_single'"
      echo "REFRESH MATERIALIZED VIEW mv_product;"
      lo=$(( 1 + (i-1)*win )); local hi=$(( lo + win - 1 ))
      echo "UPDATE tb_product SET current_price = current_price + 0.01 WHERE pk_product BETWEEN $lo AND $hi;"
      echo "\\echo '@@ update_batch'"
      echo "REFRESH MATERIALIZED VIEW mv_product;"
      echo "INSERT INTO tb_product (fk_category, fk_supplier, sku, name, base_price, current_price) VALUES (1, 1, 'NEW-$scale-$i', 'New product $i', 100, 90);"
      echo "\\echo '@@ insert_single'"
      echo "REFRESH MATERIALIZED VIEW mv_product;"
      echo "DELETE FROM tb_product WHERE sku = 'NEW-$scale-$i';"
      echo "\\echo '@@ delete_single'"
      echo "REFRESH MATERIALIZED VIEW mv_product;"
    done
  }
}

# --- parse a \timing log: pair '@@ op' markers with the next 'Time: X ms' ------
parse_log() {  # $1=logfile $2=scale $3=arm
  awk -v scale="$2" -v arm="$3" '
    /@@ / { split($0, a, "@@ "); op=a[2]; gsub(/[ \t\r]+$/, "", op); pend=op; next }
    /^Time: / && pend != "" { printf "%s\t%s\t%s\t%s\n", scale, arm, pend, $2; pend="" }
  ' "$1" >> "$raw"
}

run_arm() {  # $1=scale $2=arm(a|b|c) $3=nprod $4=scriptfile
  local scale="$1" arm="$2" nprod="$3" script="$4"
  local dbn="bench_rb_$arm"
  db "DROP DATABASE IF EXISTS $dbn"
  db "CREATE DATABASE $dbn TEMPLATE bench_rb_data"
  case "$arm" in
    a) $PSQL -d "$dbn" -c "CREATE EXTENSION jsonb_delta; CREATE EXTENSION pg_tviews;" ;;
    b) $PSQL -d "$dbn" -c "CREATE EXTENSION pg_tviews;" ;;
    c) : ;;
  esac
  local log="$outdir/${scale}_${arm}.log"
  if ! $PSQL -d "$dbn" -f "$script" >"$log" 2>&1; then
    echo "  ARM $arm FAILED (see $log):"; tail -5 "$log"; return 1
  fi
  if grep -q RB_DIVERGENCE "$log"; then
    echo "  ARM $arm CORRECTNESS FAIL:"; grep RB_DIVERGENCE "$log"; return 1
  fi
  parse_log "$log" "$scale" "$arm"
  db "DROP DATABASE IF EXISTS $dbn"
  echo "  arm $arm done"
}

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
  tview_script "$NPROD" "$scale" > "$ta"
  matview_script "$NPROD" "$scale" > "$tc"
  run_arm "$scale" a "$NPROD" "$ta" || exit 1
  run_arm "$scale" b "$NPROD" "$ta" || exit 1
  run_arm "$scale" c "$NPROD" "$tc" || exit 1
  rm -f "$ta" "$tc"
  db "DROP DATABASE IF EXISTS bench_rb_data"
done

echo
echo "=== aggregating ($raw) ==="
python3 "$here/aggregate.py" "$raw" "$outdir/summary.tsv"
