#!/usr/bin/env bash
# What one more TVIEW reading a table costs a write to it (ADR 0136, the per-TVIEW
# trigger sets). A table read by K TVIEWs gets K sets of pg_tviews triggers: each
# row trigger diffs OLD and NEW for its TVIEW, each flush trigger after the first
# finds an empty queue.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/triggers.sh [rounds] [seconds] [K ...]
#
# Variants (one database per variant and K, seeded once):
#   row    tb_x read as its own rows by K TVIEWs (row trigger, flush, truncate each)
#   delta  tb_p embedded by K TVIEWs over tb_c (delta triggers on tb_p), 1 child per parent
#   part   row, with tb_x hash-partitioned 4 ways
# Writes (pgbench, 1 client):
#   one    UPDATE of one row, a column every TVIEW reads
#   miss   UPDATE of one row, a column only the first TVIEW reads: what the other K-1
#          trigger sets cost when they have nothing to do
#   bulk   UPDATE of 100 rows, a column every TVIEW reads
# Commits do not wait for the WAL flush (synchronous_commit = off): a lone write's
# fsync would hide the CPU it costs. Rounds interleave K; the order reverses every other round. Prints one CSV line
# per run (round,variant,write,k,tps,ms), then the median ms per variant, write and K,
# and the cost of one more TVIEW: the slope of the median ms over K.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
rounds="${1:-5}"; secs="${2:-10}"; shift $(( $# > 2 ? 2 : $# ))
ks=("$@"); [[ ${#ks[@]} -eq 0 ]] && ks=(1 3 5)
variants=(${VARIANTS:-row delta part})
writes=(${WRITES:-one miss bulk})
rows=2000

schema() { # <variant> <k>
  local v="$1" k="$2" i
  echo "CREATE EXTENSION IF NOT EXISTS jsonb_delta; CREATE EXTENSION IF NOT EXISTS pg_tviews;"
  case "$v" in
  row | part)
    if [[ "$v" == part ]]; then
      echo "CREATE TABLE tb_x (pk_x int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
              a text, z text) PARTITION BY HASH (pk_x);"
      for i in 0 1 2 3; do
        echo "CREATE TABLE tb_x_$i PARTITION OF tb_x FOR VALUES WITH (MODULUS 4, REMAINDER $i);"
      done
    else
      echo "CREATE TABLE tb_x (pk_x int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
              a text, z text);"
    fi
    echo "INSERT INTO tb_x SELECT g, gen_random_uuid(), 'a' || g, 'z' || g FROM generate_series(1, $rows) g;"
    for (( i = 1; i <= k; i++ )); do
      # Only the first TVIEW reads z.
      local zcol=""; (( i == 1 )) && zcol=", 'z', x.z"
      echo "SELECT tviews.pg_tviews_create('tv_t$i', \$\$ SELECT x.pk_x AS pk_t$i, x.id,
              jsonb_build_object('id', x.id, 'a', x.a$zcol) AS data FROM tb_x x \$\$);"
    done ;;
  delta)
    echo "CREATE TABLE tb_p (pk_p int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
            a text, z text);
          CREATE TABLE tb_c (pk_c int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
            fk_p int NOT NULL REFERENCES tb_p, n text);
          CREATE INDEX ON tb_c (fk_p);
          INSERT INTO tb_p SELECT g, gen_random_uuid(), 'a' || g, 'z' || g FROM generate_series(1, $rows) g;
          INSERT INTO tb_c SELECT g, gen_random_uuid(), g, 'n' || g FROM generate_series(1, $rows) g;"
    for (( i = 1; i <= k; i++ )); do
      local zcol=""; (( i == 1 )) && zcol=", 'z', p.z"
      echo "SELECT tviews.pg_tviews_create('tv_t$i', \$\$ SELECT c.pk_c AS pk_t$i, c.id, c.fk_p,
              jsonb_build_object('id', c.id, 'n', c.n,
                'p', jsonb_build_object('id', p.id, 'a', p.a$zcol)) AS data
              FROM tb_c c JOIN tb_p p ON p.pk_p = c.fk_p \$\$);"
    done ;;
  esac
}

write_file() { # <variant> <write> -> pgbench script path
  local t=tb_x k=pk_x f
  [[ "$1" == delta ]] && { t=tb_p; k=pk_p; }
  f="$(mktemp --suffix=.sql)"
  case "$2" in
    one)  printf '\\set r random(1, %d)\nUPDATE %s SET a = md5(random()::text) WHERE %s = :r;\n' "$rows" "$t" "$k" ;;
    miss) printf '\\set r random(1, %d)\nUPDATE %s SET z = md5(random()::text) WHERE %s = :r;\n' "$rows" "$t" "$k" ;;
    bulk) printf '\\set r random(0, %d)\nUPDATE %s SET a = md5(random()::text) WHERE %s > :r * 100 AND %s <= :r * 100 + 100;\n' \
            "$(( rows / 100 - 1 ))" "$t" "$k" "$k" ;;
  esac >"$f"
  echo "$f"
}

for v in "${variants[@]}"; do
  for k in "${ks[@]}"; do
    tpl="tv_trig_${v}_$k"
    psql -X -q -d postgres -c "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $tpl" \
         -c "CREATE DATABASE $tpl" >/dev/null
    schema "$v" "$k" | psql -X -q -v ON_ERROR_STOP=1 -d "$tpl" >/dev/null
  done
done

out="$(mktemp)"; trap 'rm -f "$out"' EXIT
echo "round,variant,write,k,tps,ms"
for (( r = 1; r <= rounds; r++ )); do
  for v in "${variants[@]}"; do
    for w in "${writes[@]}"; do
      f="$(write_file "$v" "$w")"
      order=("${ks[@]}")
      (( r % 2 == 0 )) && mapfile -t order < <(printf '%s\n' "${ks[@]}" | tac)
      for k in "${order[@]}"; do
        fresh_copy "tv_trig_${v}_$k" tv_trig_run
        read -r tps ms _ < <(measure tv_trig_run "$f" "$secs" 1 "-c synchronous_commit=off")
        echo "$r,$v,$w,$k,$tps,$ms" | tee -a "$out"
      done
      rm -f "$f"
    done
  done
done

echo
echo "variant,write,k,median_ms"
awk -F, '{ key = $2 "," $3 "," $4; n[key]++; v[key, n[key]] = $6 }
  END { for (key in n) { m = n[key]; for (i = 1; i <= m; i++) a[i] = v[key, i]
          for (i = 1; i <= m; i++) for (j = i + 1; j <= m; j++) if (a[j] < a[i]) { t = a[i]; a[i] = a[j]; a[j] = t }
          med = (m % 2) ? a[(m + 1) / 2] : (a[m / 2] + a[m / 2 + 1]) / 2
          printf "%s,%.4f\n", key, med } }' "$out" | sort -t, -k1,1 -k2,2 -k3,3n | tee "$out.med"
echo
echo "variant,write,ms_at_k_min,ms_per_extra_tview,percent_of_k_min"
awk -F, '{ key = $1 "," $2; k[key, ++n[key]] = $3; ms[key, n[key]] = $4 }
  END { for (key in n) { m = n[key]; sx = sy = sxy = sxx = 0
          for (i = 1; i <= m; i++) { sx += k[key, i]; sy += ms[key, i]; sxy += k[key, i] * ms[key, i]; sxx += k[key, i] ^ 2 }
          slope = (m * sxy - sx * sy) / (m * sxx - sx ^ 2)
          printf "%s,%.4f,%.4f,%.1f%%\n", key, ms[key, 1], slope, 100 * slope / ms[key, 1] } }' "$out.med" | sort
rm -f "$out.med"
