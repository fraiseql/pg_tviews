-- A user aggregate calls its transition function for every row: a STABLE
-- function that reads a table is refused unless its reads are declared
-- (function_reads), behind an aggregate as when written as a call.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_aggregate_function_reads.sql
-- expect-output: aggregate function reads: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- Raise unless `stmt` fails with a message matching `pattern`.
CREATE FUNCTION refused(stmt text, pattern text, what text) RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
    BEGIN
        EXECUTE stmt;
    EXCEPTION WHEN OTHERS THEN
        IF SQLERRM !~ pattern THEN
            RAISE EXCEPTION 'FAIL: % failed with another error: %', what, SQLERRM;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: % was accepted', what;
END $f$;

CREATE TABLE tb_lbl (pk_lbl int PRIMARY KEY, label text);
CREATE TABLE tb_t (pk_t int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, at timestamptz);
INSERT INTO tb_lbl VALUES (1, 'one'), (2, 'two');
INSERT INTO tb_t (pk_t, name, at) VALUES (1, 'a', '2026-01-01'), (2, 'b', '2026-01-02');
CREATE TABLE tb_c (pk_c int PRIMARY KEY, fk_t int NOT NULL REFERENCES tb_t, fk_lbl int NOT NULL REFERENCES tb_lbl);
INSERT INTO tb_c VALUES (1, 1, 1), (2, 1, 2), (3, 2, 2);
CREATE FUNCTION acc(text, int) RETURNS text STABLE LANGUAGE sql AS $$
    SELECT coalesce($1, '') || (SELECT label FROM public.tb_lbl WHERE pk_lbl = $2) $$;
CREATE AGGREGATE lbl_agg(int) (sfunc = acc, stype = text);

SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_t', $$
    SELECT t.pk_t, t.id, jsonb_build_object('labels',
        (SELECT lbl_agg(c.fk_lbl ORDER BY c.pk_c) FROM tb_c c WHERE c.fk_t = t.pk_t)) AS data
    FROM tb_t t $$) $s$, 'acc.*declare them in function_reads', 'a definition calling a table-reading function through an aggregate');

SELECT 'aggregate function reads: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
