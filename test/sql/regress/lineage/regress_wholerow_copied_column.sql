-- A column copied by name into data and also read through the whole row is not
-- patched alone: the TVIEW's own row is recomputed, so the whole-row copy follows.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_wholerow_copied_column.sql
--
-- expect-output: whole-row read beside a copied column: PASS

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

CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_doc (pk_doc, title) VALUES (1, 'old');
SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('title', d.title, 'raw', to_jsonb(d)) AS data
    FROM tb_doc d $$) IS NOT NULL AS created;

UPDATE tb_doc SET title = 'new' WHERE pk_doc = 1;
SELECT assert_fresh('tv_doc', 'pk_doc', 'an UPDATE of a column copied by name and read whole');
SELECT must((SELECT data->'raw'->>'title' FROM tv_doc WHERE pk_doc = 1) = 'new', 'raw.title');

-- The plan copies no column of a table read whole.
SELECT must((SELECT plan->'direct' FROM tviews.pg_tview_meta WHERE entity = 'doc') = '[]'::jsonb,
            'tv_doc keeps a direct-patch map: '
            || (SELECT (plan->'direct')::text FROM tviews.pg_tview_meta WHERE entity = 'doc'));

SELECT 'whole-row read beside a copied column: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
