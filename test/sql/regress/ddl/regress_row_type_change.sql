-- A TVIEW reading a table whole (`to_jsonb(t)`) shows every column of the
-- table's row type: ADD COLUMN, DROP COLUMN and RENAME COLUMN change its
-- documents without a row trigger firing, and refresh it in full. A TVIEW
-- naming its columns is left alone by an added column. TABLESAMPLE, whose rows
-- change with any write, is refused.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_row_type_change.sql
--
-- expect-output: row type change: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'a'), (2, 'b');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('row', to_jsonb(i) - 'id') AS data FROM tb_item i $$) IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('tv_named', $$
    SELECT i.pk_item AS pk_named, i.id, jsonb_build_object('name', i.name) AS data FROM tb_item i $$) IS NOT NULL AS created;

ALTER TABLE tb_item ADD COLUMN size int DEFAULT 7;
SELECT assert_fresh('tv_item', 'pk_item', 'ADD COLUMN on a table read whole');
DO $$ BEGIN
    IF (SELECT data->'row'->>'size' FROM tv_item WHERE pk_item = 1) IS DISTINCT FROM '7' THEN
        RAISE EXCEPTION 'FAIL: the added column is missing from tv_item: %',
            (SELECT data FROM tv_item WHERE pk_item = 1);
    END IF;
END $$;

ALTER TABLE tb_item RENAME COLUMN size TO weight;
SELECT assert_fresh('tv_item', 'pk_item', 'RENAME COLUMN on a table read whole');

ALTER TABLE tb_item DROP COLUMN weight;
SELECT assert_fresh('tv_item', 'pk_item', 'DROP COLUMN on a table read whole');
SELECT assert_fresh('tv_named', 'pk_named', 'the column changes, on a TVIEW naming its columns');

-- Writes after the change still refresh.
UPDATE tb_item SET name = 'z' WHERE pk_item = 2;
SELECT assert_fresh('tv_item', 'pk_item', 'an UPDATE after the row type changed');

DO $$ BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_create('tv_sample', $q$
            SELECT i.pk_item AS pk_sample, i.id, jsonb_build_object('name', i.name) AS data
            FROM tb_item i TABLESAMPLE SYSTEM (50) $q$);
    EXCEPTION WHEN feature_not_supported THEN
        IF SQLERRM !~ 'TABLESAMPLE' THEN
            RAISE EXCEPTION 'FAIL: TABLESAMPLE refused with another message: %', SQLERRM;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: a definition reading a table with TABLESAMPLE was accepted';
END $$;

SELECT 'row type change: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
