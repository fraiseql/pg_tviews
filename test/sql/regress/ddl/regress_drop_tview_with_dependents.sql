-- Regression test (#47): DROP TABLE tv_* honours CASCADE and RESTRICT.
--
-- The ProcessUtility hook dropped the TVIEW table with a hard-coded RESTRICT
-- drop, so DROP TABLE tv_x CASCADE on a TVIEW a user view reads failed with an
-- opaque "PANIC ... Any { .. }", and a plain DROP degraded to the same message.
-- Now CASCADE removes the TVIEW, its registration and the dependent view, and
-- RESTRICT fails with PostgreSQL's own 2BP01 error.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_drop_tview_with_dependents.sql
-- expect-output: drop_tview_with_dependents: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#47 FAIL: %', what; END IF; END $$;

CREATE TABLE tb_cascade_post (
    pk_cascade_post bigint PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    title text
);
INSERT INTO tb_cascade_post (pk_cascade_post, title) VALUES (1, 'a'), (2, 'b');

SELECT pg_tviews_create('tv_cascade_post', $$
    SELECT pk_cascade_post, id, jsonb_build_object('title', title) AS data FROM tb_cascade_post $$);
CREATE VIEW dependent_report AS SELECT pk_cascade_post, data FROM tv_cascade_post;

-- 1. RESTRICT: a clean dependency error, not the opaque panic; nothing dropped.
DO $$
DECLARE msg text;
BEGIN
    BEGIN
        DROP TABLE public.tv_cascade_post;
        RAISE EXCEPTION '#47 FAIL: RESTRICT drop succeeded despite dependent_report';
    EXCEPTION WHEN dependent_objects_still_exist THEN
        GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT;
        IF msg LIKE '%Any { .. }%' THEN
            RAISE EXCEPTION '#47 FAIL: degraded panic message instead of a clean error: %', msg;
        END IF;
    END;
END $$;
SELECT must(to_regclass('public.tv_cascade_post') IS NOT NULL
            AND EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'cascade_post'),
            'a refused RESTRICT drop removed the TVIEW');

-- 2. CASCADE: the TVIEW, its dependent view and its registration are gone.
DROP TABLE public.tv_cascade_post CASCADE;
SELECT must(to_regclass('public.tv_cascade_post') IS NULL, 'tv_cascade_post survived CASCADE');
SELECT must(to_regclass('public.dependent_report') IS NULL, 'dependent_report survived CASCADE');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'cascade_post'),
            'pg_tview_meta row for cascade_post survived CASCADE');

DROP EXTENSION pg_tviews CASCADE;
\echo 'drop_tview_with_dependents: PASS'
