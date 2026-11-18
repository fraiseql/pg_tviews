-- Reading the clock is reading the clock, however it is written: 'now' cast
-- from text at run time reads the time as now() does, and is refused unless the
-- TVIEW declares time_refresh.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_time_through_text_cast.sql
-- expect-output: time through a text cast: PASS

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


SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_t', $$
    SELECT t.pk_t, t.id, jsonb_build_object('past', t.at < 'now'::text::timestamptz) AS data FROM tb_t t $$) $s$, 'reads the time.*declare time_refresh', 'a definition reading the time through a cast from text');

SELECT 'time through a text cast: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
