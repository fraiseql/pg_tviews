-- Rows aggregated whole (jsonb_agg(i)) are read whole: their parent refreshes on an
-- UPDATE of any column of them.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_wholerow_aggregate.sql
--
-- expect-output: whole rows aggregated: PASS

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

CREATE TABLE tb_box (pk_box int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_box int NOT NULL REFERENCES tb_box, label text);
INSERT INTO tb_box (pk_box, name) VALUES (1, 'b');
INSERT INTO tb_item (pk_item, fk_box, label) VALUES (5, 1, 'old'), (6, 1, 'z');
SELECT tviews.pg_tviews_create('tv_box', $$
    SELECT b.pk_box, b.id, jsonb_build_object('name', b.name,
        'items', (SELECT jsonb_agg(i ORDER BY i.pk_item) FROM tb_item i WHERE i.fk_box = b.pk_box)) AS data
    FROM tb_box b $$) IS NOT NULL AS created;

UPDATE tb_item SET label = 'new' WHERE pk_item = 5;
SELECT assert_fresh('tv_box', 'pk_box', 'an UPDATE of a column of a row aggregated whole');

SELECT must(stopgap('wholerow') IS NULL OR stopgap('wholerow') > 0, 'no stopgap:wholerow counted');

SELECT 'whole rows aggregated: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
