-- Regression test (#246): the report orders and reads rows by their integer key.
--
-- pg_tviews_flush_and_report() reports rows by entity, then pk_<entity>. The key
-- is BIGINT: 9 comes before 10 and 100, and a key past 2^53 (beyond a JSON
-- double) keeps its document and its place.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_report_integer_keys.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- The id's last group names the row: 9, 10, 100, and f… for the largest key.
CREATE FUNCTION tag(e jsonb) RETURNS text LANGUAGE sql AS $$
    SELECT ltrim(split_part(e->>'id', '-', 5), '0') $$;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'report integer keys FAIL: %', what; END IF; END $$;

CREATE TABLE tb_tag (pk_tag bigint PRIMARY KEY, id uuid NOT NULL, name text NOT NULL);
SELECT tviews.pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, t.id, jsonb_build_object('name', t.name) AS data FROM tb_tag t $$);

BEGIN;
INSERT INTO tb_tag VALUES (100, '00000000-0000-0000-0000-000000000100', 'hundred'),
                          (9, '00000000-0000-0000-0000-000000000009', 'nine'),
                          (9223372036854775806, 'ffffffff-ffff-ffff-ffff-ffffffffffff', 'max'),
                          (10, '00000000-0000-0000-0000-000000000010', 'ten');
SELECT tviews.pg_tviews_flush_and_report(include_data => true) AS r \gset
SELECT must((SELECT array_agg(tag(e) ORDER BY n)
             FROM jsonb_array_elements((:'r'::jsonb)->'updated') WITH ORDINALITY AS x(e, n))
            = ARRAY['9', '10', '100', 'ffffffffffff'], 'order: ' || :'r');
SELECT must((SELECT e->'data'->>'name' FROM jsonb_array_elements((:'r'::jsonb)->'updated') e
             WHERE tag(e) = 'ffffffffffff') = 'max', 'largest key lost its document: ' || :'r');
COMMIT;

BEGIN;
DELETE FROM tb_tag WHERE pk_tag IN (10, 9223372036854775806);
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must((SELECT array_agg(tag(e) ORDER BY n)
             FROM jsonb_array_elements((:'r'::jsonb)->'deleted') WITH ORDINALITY AS x(e, n))
            = ARRAY['10', 'ffffffffffff'], 'deleted: ' || :'r');
COMMIT;

\echo 'regress_report_integer_keys: PASS'
