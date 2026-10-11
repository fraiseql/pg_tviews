-- A catalog row restored before the tables its plan names, or naming one that is
-- gone, is refused with 42P01 and a hint (pg_tviews_meta_rebind, #212): the row
-- would bind its plan to nothing.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_restore_missing_table.sql
-- expect-output: restore_missing_table: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'restore_missing_table FAIL: %', what; END IF; END $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
SELECT tviews.pg_tviews_create('tv_item',
    $$SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item$$);

-- The row as a dump holds it, then its registration gone (the TVIEW dropped).
CREATE TABLE saved AS SELECT * FROM tviews.pg_tview_meta WHERE entity = 'item';
SELECT tviews.pg_tviews_drop('item');

-- Its plan names a table that does not exist: refused, with the table and a hint.
UPDATE saved SET plan = jsonb_set(plan, '{tables,0,table}', '"public.tb_gone"');
DO $$
DECLARE state text; message text; hint text;
BEGIN
    INSERT INTO tviews.pg_tview_meta SELECT * FROM saved;
    PERFORM must(false, 'a row naming a missing table was restored');
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, message = MESSAGE_TEXT,
                            hint = PG_EXCEPTION_HINT;
    PERFORM must(state = '42P01', 'SQLSTATE ' || state || ': ' || message);
    PERFORM must(message LIKE '%tv_item%public.tb_gone%', 'message: ' || message);
    PERFORM must(hint LIKE '%Restore the tables%', 'hint: ' || hint);
END $$;
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'item'),
            'the refused row was kept');

\echo 'restore_missing_table: PASS'
