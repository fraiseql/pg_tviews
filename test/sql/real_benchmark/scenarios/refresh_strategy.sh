#!/usr/bin/env bash
# Refresh strategy crossover (issue #77, docs/adr/0077-refresh-strategy-selection.md).
#
# Times the statements a flush could use to bring N changed TVIEW rows up to date,
# on plain tables shaped like a TVIEW (no pg_tviews involved), so only the SQL
# strategy varies:
#   S1   INSERT … SELECT … FROM v WHERE pk = ANY($1) ON CONFLICT … (today; chunked)
#   S2   the same, but JOIN unnest($1) instead of = ANY($1)
#   S3   UNLOGGED staging table + MERGE
#   S4   full rebuild (TRUNCATE + INSERT … SELECT)
# over N changed keys, clustered (contiguous pks) or scattered (random pks).
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./refresh_strategy.sh [rows] [persistence]
#   rows         table size (default 200000)
#   persistence  UNLOGGED (default) or LOGGED
# Prints TSV: persistence layout n strategy median_ms blocks

set -euo pipefail
ROWS="${1:-200000}"
PERSIST="${2:-UNLOGGED}"
[[ "$PERSIST" == LOGGED ]] && PERSIST=""
DB="pgtv_strategy_$$"
psql -qX -d postgres -c "CREATE DATABASE $DB" >/dev/null
trap 'psql -qX -d postgres -c "DROP DATABASE IF EXISTS $DB" >/dev/null' EXIT

psql -qX -v ON_ERROR_STOP=1 -d "$DB" >/dev/null <<SQL
CREATE TABLE tb_item (pk_item BIGINT PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid(),
                      n INT NOT NULL, payload TEXT NOT NULL);
INSERT INTO tb_item (pk_item, n, payload)
SELECT g, 0, repeat(md5(g::text), 30) FROM generate_series(1, $ROWS) g;
CREATE VIEW v_item AS
SELECT pk_item, id, jsonb_build_object('n', n, 'payload', payload) AS data FROM tb_item;
CREATE $PERSIST TABLE tv_item (pk_item BIGINT PRIMARY KEY, id UUID NOT NULL, data JSONB,
                               updated_at TIMESTAMPTZ NOT NULL DEFAULT now())
    WITH (fillfactor = 85);
INSERT INTO tv_item (pk_item, id, data) SELECT * FROM v_item;
CREATE UNLOGGED TABLE stage (LIKE tv_item INCLUDING DEFAULTS);
VACUUM ANALYZE tb_item, tv_item;

-- One strategy run over the given keys; returns milliseconds.
CREATE FUNCTION run(strategy TEXT, keys BIGINT[], batch INT) RETURNS NUMERIC LANGUAGE plpgsql AS \$\$
DECLARE t0 TIMESTAMPTZ; i INT;
BEGIN
    UPDATE tb_item SET n = n + 1 WHERE pk_item = ANY(keys);   -- make every key a real change
    t0 := clock_timestamp();
    IF strategy = 'S1' THEN
        FOR i IN 0 .. (coalesce(array_length(keys, 1), 0) - 1) / batch LOOP
            INSERT INTO tv_item (pk_item, id, data)
            SELECT pk_item, id, data FROM v_item WHERE pk_item = ANY(keys[i * batch + 1 : (i + 1) * batch])
            ON CONFLICT (pk_item) DO UPDATE SET id = EXCLUDED.id, data = EXCLUDED.data, updated_at = now()
            WHERE (tv_item.id, tv_item.data) IS DISTINCT FROM (EXCLUDED.id, EXCLUDED.data);
        END LOOP;
    ELSIF strategy = 'S2' THEN
        INSERT INTO tv_item (pk_item, id, data)
        SELECT v.pk_item, v.id, v.data FROM v_item v JOIN unnest(keys) k(pk) ON v.pk_item = k.pk
        ON CONFLICT (pk_item) DO UPDATE SET id = EXCLUDED.id, data = EXCLUDED.data, updated_at = now()
        WHERE (tv_item.id, tv_item.data) IS DISTINCT FROM (EXCLUDED.id, EXCLUDED.data);
    ELSIF strategy = 'S3' THEN
        TRUNCATE stage;
        INSERT INTO stage (pk_item, id, data)
        SELECT v.pk_item, v.id, v.data FROM v_item v JOIN unnest(keys) k(pk) ON v.pk_item = k.pk;
        MERGE INTO tv_item t USING stage s ON t.pk_item = s.pk_item
        WHEN MATCHED AND (t.id, t.data) IS DISTINCT FROM (s.id, s.data)
            THEN UPDATE SET id = s.id, data = s.data, updated_at = now()
        WHEN NOT MATCHED THEN INSERT (pk_item, id, data) VALUES (s.pk_item, s.id, s.data);
    ELSE
        TRUNCATE tv_item;
        INSERT INTO tv_item (pk_item, id, data) SELECT pk_item, id, data FROM v_item;
    END IF;
    RETURN extract(epoch FROM clock_timestamp() - t0) * 1000;
END \$\$;
SQL

keys_sql() { # <layout> <n>
  if [[ "$1" == clustered ]]; then
    echo "ARRAY(SELECT g::bigint FROM generate_series(1000, 999 + $2) g)"
  else
    echo "ARRAY(SELECT g::bigint FROM generate_series(1, $ROWS) g ORDER BY random() LIMIT $2)"
  fi
}

persist_label="${PERSIST:-LOGGED}"
for layout in clustered scattered; do
  for n in 10 100 1000 10000 100000; do
    (( n > ROWS )) && continue
    blocks=$(psql -qAtX -d "$DB" -c "SELECT count(DISTINCT (ctid::text::point)[0]) FROM tv_item WHERE pk_item = ANY($(keys_sql "$layout" "$n"))")
    for spec in "S1:100" "S1:1000" "S1:10000" "S1:1000000" "S2:0" "S3:0" "S4:0"; do
      strategy="${spec%%:*}"; batch="${spec##*:}"
      label="$strategy"; [[ "$strategy" == S1 ]] && label="S1/b$batch"
      [[ "$strategy" == S1 && "$batch" == 1000000 ]] && label="S1/ball"
      # S4 is independent of N past the first size: skip it for the larger layouts' repeats.
      ms=$(psql -qAtX -d "$DB" -c "
        SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY ms)
        FROM (SELECT run('$strategy', k, greatest($batch, 1)) AS ms
              FROM (SELECT $(keys_sql "$layout" "$n") AS k) s, generate_series(1, 3)) r")
      printf '%s\t%s\t%s\t%s\t%.1f\t%s\n' "$persist_label" "$layout" "$n" "$label" "$ms" "$blocks"
    done
  done
done
