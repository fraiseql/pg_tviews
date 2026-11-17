-- A TVIEW reading a system column of a table (xmin, ctid, tableoid) refreshes on
-- every UPDATE of the table: each UPDATE changes the row's system columns.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_system_column_read.sql
--
-- expect-output: system column read: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- The refreshes forced because the definition reads a table whole, by reason
-- (test-hooks builds; NULL in others).
CREATE FUNCTION stopgap(reason text) RETURNS bigint LANGUAGE sql AS $$
    SELECT CASE WHEN tviews.pg_tviews_show_queue() ? 'patch_stats'
                THEN coalesce((tviews.pg_tviews_show_queue()->'patch_stats'->>('stopgap:' || reason))::bigint, 0)
           END $$;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

CREATE TABLE tb_v (pk_v int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, note text);
INSERT INTO tb_v (pk_v, name) VALUES (1, 'n'), (2, 'm');
SELECT tviews.pg_tviews_create('tv_v', $$
    SELECT v.pk_v, v.id, jsonb_build_object('name', v.name, 'version', v.xmin::text) AS data
    FROM tb_v v $$) IS NOT NULL AS created;

-- A child table read through ctid, on the statement path.
CREATE TABLE tb_vc (pk_vc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                    fk_v int NOT NULL REFERENCES tb_v, note text);
INSERT INTO tb_vc (pk_vc, fk_v) VALUES (1, 1), (2, 1);
SELECT tviews.pg_tviews_create('tv_vp', $$
    SELECT v.pk_v AS pk_vp, v.id, jsonb_build_object('places',
        (SELECT jsonb_agg(c.ctid::text ORDER BY c.pk_vc) FROM tb_vc c WHERE c.fk_v = v.pk_v)) AS data
    FROM tb_v v $$) IS NOT NULL AS created;

UPDATE tb_v SET note = 'x' WHERE pk_v = 1;
SELECT assert_fresh('tv_v', 'pk_v', 'an UPDATE of a column the TVIEW reads only through xmin');
UPDATE tb_vc SET note = 'y' WHERE pk_vc = 2;
SELECT assert_fresh('tv_vp', 'pk_vp', 'an UPDATE of a child row read only through ctid');

SELECT must(stopgap('syscol') IS NULL OR stopgap('syscol') > 0, 'no stopgap:syscol counted');

SELECT 'system column read: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
