-- A child collection built from the whole rows of a child table refreshes its
-- parent on an UPDATE of any column of a child row.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_wholerow_child_collection.sql
--
-- expect-output: whole-row child collection: PASS

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

CREATE TABLE tb_holder (pk_holder int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_hline (pk_hline int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_holder int NOT NULL REFERENCES tb_holder, label text);
INSERT INTO tb_holder (pk_holder, name) VALUES (1, 'h1'), (2, 'h2');
INSERT INTO tb_hline (pk_hline, fk_holder, label) VALUES (10, 1, 'old'), (11, 1, 'x'), (20, 2, 'y');
SELECT tviews.pg_tviews_create('tv_holder', $$
    SELECT h.pk_holder, h.id, jsonb_build_object('name', h.name,
        'lines', (SELECT jsonb_agg(to_jsonb(l) ORDER BY l.pk_hline) FROM tb_hline l
                  WHERE l.fk_holder = h.pk_holder)) AS data
    FROM tb_holder h $$) IS NOT NULL AS created;

UPDATE tb_hline SET label = 'new' WHERE pk_hline = 10;
SELECT assert_fresh('tv_holder', 'pk_holder', 'an UPDATE of a child column read only through the whole row');
-- Moving a child row to another parent refreshes both.
UPDATE tb_hline SET fk_holder = 2 WHERE pk_hline = 11;
SELECT assert_fresh('tv_holder', 'pk_holder', 'a child row moved to another parent');

SELECT must(stopgap('wholerow') IS NULL OR stopgap('wholerow') > 0, 'no stopgap:wholerow counted');

SELECT 'whole-row child collection: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
