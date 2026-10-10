#!/usr/bin/env bash
# ADR 0157: cost of mapping base-table writes to TVIEW keys, on the installed build.
#
# The ADR's shape: 20k orders, 200k lines, 2k SKUs; tv_order lists the SKU names of
# its lines (tb_sku reaches tv_order through tb_line: two hops). Each case is an
# autocommit statement timed by psql \timing (so it includes the refresh flush);
# the median of its iterations is printed as `case<TAB>median_ms<TAB>iterations`.
# The run gates on tv_order matching its backing view.
#
# Run it once per build to compare (e.g. the previous release, then this one):
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./scenarios/adr_0157_mapping.sh

set -euo pipefail
export PGHOST="${PGHOST:-localhost}" PGPORT="${PGPORT:-28818}" PGUSER="${PGUSER:-postgres}"
db="pg_tviews_bench_0157_$$"
work="$(mktemp -d)"
trap 'psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1; rm -rf "$work"' EXIT

psql -X -q -d postgres -c "CREATE DATABASE $db" >/dev/null
psql -X -q -d "$db" -v ON_ERROR_STOP=1 >/dev/null <<'SQL'
SET client_min_messages TO WARNING;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), ref text);
CREATE TABLE tb_sku (pk_sku bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_line (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_order bigint NOT NULL, fk_sku bigint NOT NULL, pos int NOT NULL);
INSERT INTO tb_order (pk_order, ref) SELECT g, 'o' || g FROM generate_series(1, 20000) g;
INSERT INTO tb_sku (pk_sku, name) SELECT g, 's' || g FROM generate_series(1, 2000) g;
INSERT INTO tb_line (pk_line, fk_order, fk_sku, pos)
    SELECT g, 1 + g % 20000, 1 + (g * 7) % 2000, g % 10 FROM generate_series(1, 200000) g;
CREATE INDEX ON tb_line (fk_order);
CREATE INDEX ON tb_line (fk_sku);
ANALYZE;
SELECT tviews.pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           jsonb_build_object('ref', o.ref,
               'skus', COALESCE(jsonb_agg(s.name ORDER BY l.pos, l.pk_line)
                                FILTER (WHERE s.pk_sku IS NOT NULL), '[]')) AS data
    FROM tb_order o
    LEFT JOIN tb_line l ON l.fk_order = o.pk_order
    LEFT JOIN tb_sku s ON s.pk_sku = l.fk_sku
    GROUP BY o.pk_order, o.id, o.ref $$);
SQL

# case name, iterations, statement (%d is replaced by the iteration number)
run_case() {
    local name="$1" iterations="$2" statement="$3" i
    {
        # A 100k-row statement queues every order.
        echo 'SET pg_tviews.max_queue_size = 1000000;'
        echo '\timing on'
        for ((i = 1; i <= iterations; i++)); do
            printf "${statement}\n" "$i"
        done
    } > "$work/$name.sql"
    psql -X -q -d "$db" -v ON_ERROR_STOP=1 -f "$work/$name.sql" \
        | sed -n 's/^Time: \([0-9.]*\) ms.*/\1/p' | sort -n > "$work/$name.ms"
    printf '%s\t%s\t%s\n' "$name" \
        "$(awk '{a[NR]=$1} END {print (NR % 2) ? a[(NR+1)/2] : (a[NR/2] + a[NR/2+1]) / 2}' "$work/$name.ms")" \
        "$iterations"
}

printf 'case\tmedian_ms\titerations\n'
run_case root_update_1      30 "UPDATE tb_order SET ref = ref || '+' WHERE pk_order = %d * 97;"
run_case line_update_1      30 "UPDATE tb_line SET pos = pos + 1 WHERE pk_line = %d * 977;"
run_case sku_update_1       30 "UPDATE tb_sku SET name = name || '+' WHERE pk_sku = %d * 13;"
run_case line_update_100k    3 "UPDATE tb_line SET pos = pos + 1 WHERE pk_line %% 2 = %d %% 2;"
run_case sku_update_1000     3 "UPDATE tb_sku SET name = name || '+' WHERE pk_sku %% 2 = %d %% 2;"

diverging="$(psql -X -At -d "$db" -c "SELECT count(*) FROM tv_order t FULL JOIN tviews.public__tv_order v USING (pk_order)
                                      WHERE t.data IS DISTINCT FROM v.data")"
if [[ "$diverging" != 0 ]]; then
    echo "FAIL: tv_order diverges from its backing view ($diverging rows)" >&2
    exit 1
fi
