#!/usr/bin/env bash
# Self-check for lib/explain_on.sql + lib/explain_extract.py: one single-row
# refresh must yield an extracted plan for the tv_ write carrying "WAL Records".
# $1 = scratch database (created and dropped by selftest.sh)
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
db="$1"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

psql -X -v ON_ERROR_STOP=1 -d "$db" >"$tmp/log" 2>&1 <<SQL || { tail -3 "$tmp/log"; exit 1; }
SET client_min_messages TO WARNING;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v int);
INSERT INTO tb_item (pk_item, v) SELECT g, 0 FROM generate_series(1, 10) g;
SELECT pg_tviews_create('tv_item',
  'SELECT pk_item, id, jsonb_build_object(''v'', v) AS data FROM tb_item');
\i $here/explain_on.sql
UPDATE tb_item SET v = v + 1 WHERE pk_item = 1;
SQL

python3 "$here/explain_extract.py" "$tmp/log" "$tmp/plans" --match 'tv_item' >/dev/null || exit 1
grep -l '"WAL Records"' "$tmp"/plans/*.json 2>/dev/null \
  | xargs -r grep -lE '"Query Text": "(INSERT INTO|UPDATE) [^"]*tv_item' | grep -q . \
  || { echo "no tv_item write plan with WAL Records"; exit 1; }
echo EXPLAIN_SELFTEST_OK
