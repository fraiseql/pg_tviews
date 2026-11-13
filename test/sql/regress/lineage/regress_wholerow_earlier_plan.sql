-- A TVIEW reading a table whole whose stored plan is the one an earlier release
-- derived, not derived again: it still refreshes on an UPDATE of a column read
-- only through the whole row, and a column also copied by name is not patched.
-- Whether a write may skip or patch is decided from the definition at the time
-- of the write, not only from the stored plan.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_wholerow_earlier_plan.sql
--
-- expect-output: whole-row read under an earlier plan: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     title text, note text);
INSERT INTO tb_doc (pk_doc, title) VALUES (1, 'old');
SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('title', d.title, 'raw', to_jsonb(d)) AS data
    FROM tb_doc d $$) IS NOT NULL AS created;

-- The plan as the earlier release stored it: title is a copy alone, and links
-- are the key only. Not needs_reregister: nothing says the plan is old.
UPDATE tviews.pg_tview_meta
   SET plan = jsonb_set(jsonb_set(plan, '{direct}', '[["title", "title"]]'),
                        '{tables,0,links}', '[1]')
 WHERE entity = 'doc';
SELECT must(plan->'direct' = '[["title", "title"]]'::jsonb, 'the earlier plan was not stored')
  FROM tviews.pg_tview_meta WHERE entity = 'doc';

UPDATE tb_doc SET note = 'n' WHERE pk_doc = 1;
SELECT assert_fresh('tv_doc', 'pk_doc', 'an UPDATE of a column read only through the whole row');
UPDATE tb_doc SET title = 'new' WHERE pk_doc = 1;
SELECT assert_fresh('tv_doc', 'pk_doc', 'an UPDATE of a column copied by name and read whole');

SELECT 'whole-row read under an earlier plan: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
