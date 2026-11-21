-- Regression test: a domain between a column and its copy in data.
--   1. A column whose type is a domain, copied bare: to_jsonb of the domain value is
--      to_jsonb of its base value, so the row is patched, and stays fresh.
--   2. A column cast to a domain in the definition (a CoerceToDomain over the Var):
--      not a plain copy (the domain's check runs, and may refuse the value), so the
--      row is recomputed, not patched; a value the domain refuses fails the write.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/direct_patch/regress_domain_copy.sql
-- expect-output: domain copy: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'domain copy FAIL: %', what; END IF; END $$;
CREATE FUNCTION applied() RETURNS bigint LANGUAGE sql AS
    $$ SELECT (tviews.pg_tviews_show_queue()->>'direct_patches_applied')::bigint $$;
CREATE FUNCTION fresh(e text) RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE stale boolean;
BEGIN
    EXECUTE format('SELECT EXISTS (SELECT pk_%1$s, data FROM tv_%1$s EXCEPT
                                   SELECT pk_%1$s, data FROM tviews.public__tv_%1$s)', e)
       INTO stale;
    RETURN NOT stale;
END $$;

CREATE DOMAIN label AS text CHECK (VALUE <> '');
CREATE DOMAIN short_name AS text CHECK (length(VALUE) <= 5);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      tag label NOT NULL, name text NOT NULL);
INSERT INTO tb_item VALUES (1, DEFAULT, 'a', 'one');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('tag', tag, 'name', name::short_name) AS data
    FROM tb_item $$);

-- 1.
BEGIN;
SELECT applied() AS before \gset
UPDATE tb_item SET tag = 'b' WHERE pk_item = 1;
SELECT must(applied() = :before + 1, 'the domain-typed column was not patched');
COMMIT;
SELECT must(fresh('item'), 'stale after the domain-typed column');

-- 2.
BEGIN;
SELECT applied() AS before \gset
UPDATE tb_item SET name = 'two' WHERE pk_item = 1;
SELECT must(applied() = :before, 'the column cast to a domain was patched');
COMMIT;
SELECT must(fresh('item'), 'stale after the column cast to a domain');
\set ON_ERROR_STOP off
UPDATE tb_item SET name = 'longer than five' WHERE pk_item = 1;
-- expect-output: violates check constraint "short_name_check"
\set ON_ERROR_STOP on
SELECT must(fresh('item'), 'stale after the refused value');

\echo 'domain copy: PASS'
