-- A TVIEW's rows are the same for every reader: a definition that reads who
-- reads it (CURRENT_USER, CURRENT_ROLE, USER, current_user()) is refused with
-- 0A000, at any level of its view chain. Its refresh runs as its owner, so the
-- rows would name the owner, not the reader; a policy on tv_* filters readers.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_reader_functions_refused.sql
--
-- expect-output: reader functions refused: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- Raise unless `stmt` fails with SQLSTATE `want` and a message matching `like`.
CREATE FUNCTION refused(stmt text, want text, like_ text, what text) RETURNS void
LANGUAGE plpgsql AS $f$
DECLARE state text; msg text;
BEGIN
    BEGIN
        EXECUTE stmt;
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, msg = MESSAGE_TEXT;
        IF state <> want OR msg NOT LIKE like_ THEN
            RAISE EXCEPTION 'FAIL: % failed with % (want %): %', what, state, want, msg;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: % was accepted', what;
END $f$;

CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     owner_name text, title text);
INSERT INTO tb_doc (pk_doc, owner_name, title) VALUES (1, 'postgres', 'd1');

SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('title', d.title, 'me', CURRENT_USER) AS data
    FROM tb_doc d $$) $s$, '0A000', '%reads CURRENT_USER%', 'CURRENT_USER in the data');
SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('title', d.title) AS data
    FROM tb_doc d WHERE d.owner_name = CURRENT_ROLE $$) $s$, '0A000', '%reads CURRENT_ROLE%',
    'CURRENT_ROLE in a filter');
SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('mine', d.owner_name = USER) AS data
    FROM tb_doc d $$) $s$, '0A000', '%reads USER%', 'USER');
SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('me', pg_catalog."current_user"()) AS data
    FROM tb_doc d $$) $s$, '0A000', '%reads current_user%', 'current_user() called');
-- In a subquery, and in a view the definition reads.
SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('n', (SELECT count(*) FROM tb_doc e
                                                    WHERE e.owner_name = CURRENT_USER)) AS data
    FROM tb_doc d $$) $s$, '0A000', '%reads CURRENT_USER%', 'CURRENT_USER in a subquery');
CREATE VIEW v_my_docs AS SELECT * FROM tb_doc WHERE owner_name = CURRENT_USER;
SELECT refused($s$ SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('title', d.title) AS data
    FROM v_my_docs d $$) $s$, '0A000', '%reads CURRENT_USER%', 'CURRENT_USER in a view read');
-- So does CREATE TABLE tv_* AS.
SELECT refused($s$ CREATE TABLE tv_doc AS
    SELECT d.pk_doc, d.id, jsonb_build_object('me', CURRENT_USER) AS data FROM tb_doc d $s$,
    '0A000', '%reads CURRENT_USER%', 'CURRENT_USER under CREATE TABLE AS');
DO $$ BEGIN
    IF to_regclass('tv_doc') IS NOT NULL OR EXISTS (SELECT 1 FROM tviews.pg_tview_meta) THEN
        RAISE EXCEPTION 'FAIL: a refused definition left a TVIEW behind';
    END IF;
END $$;

-- The definition without them is accepted (SESSION_USER, which names the
-- session, is refused as such: regress_dialect_refusals).
SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('title', d.title) AS data FROM tb_doc d $$)
    IS NOT NULL AS created;

SELECT 'reader functions refused: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
