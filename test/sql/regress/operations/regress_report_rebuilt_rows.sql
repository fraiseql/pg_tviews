-- Regression test: a report names no row that a later rebuild in the same
-- transaction may have removed.
--
-- A rebuild (or refill) replaces a TVIEW's rows without journaling them one by
-- one: its type is reported in invalidated_types, and the change feed publishes
-- it by name (`t`) without its rows. Without data, the report listed the rows
-- journaled before the rebuild too, which may no longer exist. The type now
-- stands for all its rows, as it does for its collections. With data, each row is
-- read as it is now, and one the rebuild removed is left out (unchanged).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_report_rebuilt_rows.sql
--
-- expect-output: regress_report_rebuilt_rows: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_item (
    pk_item int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name    text NOT NULL
);
CREATE TABLE tb_tag (
    pk_tag int PRIMARY KEY,
    id     uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name   text NOT NULL
);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'a'), (2, 'b');
INSERT INTO tb_tag (pk_tag, name) VALUES (1, 't');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item $$);
SELECT tviews.pg_tviews_create('tv_tag', $$
    SELECT pk_tag, id, jsonb_build_object('name', name) AS data FROM tb_tag $$);

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF ok IS NOT TRUE THEN RAISE EXCEPTION 'regress_report_rebuilt_rows FAIL: %', what; END IF;
END $$;

-- Row 2 is updated, then deleted without a cascade, then the TVIEW is rebuilt:
-- row 2 is gone. tv_tag is written alongside and keeps its row.
CREATE FUNCTION scenario(include_data boolean) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE report jsonb;
BEGIN
    UPDATE tb_item SET name = 'b2' WHERE pk_item = 2;
    UPDATE tb_tag SET name = 't2' WHERE pk_tag = 1;
    PERFORM tviews.pg_tviews_suspend_triggers();
    DELETE FROM tb_item WHERE pk_item = 2;
    PERFORM tviews.pg_tviews_resume_triggers();
    PERFORM tviews.pg_tviews_refresh('item');
    report := tviews.pg_tviews_flush_and_report(include_data => include_data);
    RAISE EXCEPTION USING ERRCODE = 'P0099', MESSAGE = report::text;
EXCEPTION WHEN SQLSTATE 'P0099' THEN
    RETURN SQLERRM::jsonb;
END $$;

-- Without data: no Item row, Item invalidated, the Tag row still listed.
SELECT scenario(false) AS r \gset
SELECT must(NOT EXISTS (SELECT FROM jsonb_array_elements(:'r'::jsonb->'updated') e
                        WHERE e->>'__typename' = 'Item'),
            'without data, a rebuilt type''s rows are listed: ' || :'r');
SELECT must(:'r'::jsonb->'invalidated_types' = '["Item"]', 'invalidated_types: ' || :'r');
SELECT must((SELECT count(*) FROM jsonb_array_elements(:'r'::jsonb->'updated') e
             WHERE e->>'__typename' = 'Tag') = 1, 'the Tag row: ' || :'r');

-- With data: the same (row 2 is read as gone).
SELECT scenario(true) AS r \gset
SELECT must(NOT EXISTS (SELECT FROM jsonb_array_elements(:'r'::jsonb->'updated') e
                        WHERE e->>'__typename' = 'Item'),
            'with data, a removed row is listed: ' || :'r');
SELECT must(:'r'::jsonb->'invalidated_types' = '["Item"]', 'invalidated_types (data): ' || :'r');
SELECT must((SELECT count(*) FROM jsonb_array_elements(:'r'::jsonb->'updated') e
             WHERE e->>'__typename' = 'Tag' AND e->'data'->>'name' = 't2') = 1,
            'the Tag row (data): ' || :'r');

\echo 'regress_report_rebuilt_rows: PASS'
