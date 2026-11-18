-- A user operator calls its function as a call would: a STABLE function that
-- reads a table is refused unless its reads are declared (function_reads),
-- written as an operator as when written as a call.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_operator_function_reads.sql
-- expect-output: operator function reads: PASS

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
CREATE FUNCTION lbl_of(int, int) RETURNS text STABLE LANGUAGE sql AS $$
    SELECT label FROM public.tb_lbl WHERE pk_lbl = $1 $$;
CREATE OPERATOR ### (leftarg = int, rightarg = int, function = lbl_of);

SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_t', $$
    SELECT t.pk_t, t.id, jsonb_build_object('label', t.pk_t ### 0) AS data FROM tb_t t $$) $s$, 'lbl_of.*declare them in function_reads', 'a definition calling a table-reading function through an operator');

SELECT 'operator function reads: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
