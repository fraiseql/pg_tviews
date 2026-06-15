-- Regression test for issue #47: DROP TABLE tv_* CASCADE panics in ProcessUtility hook
--
-- The ProcessUtility hook intercepts DROP TABLE tv_* and dropped the table via an
-- internal, hard-coded non-CASCADE `DROP TABLE IF EXISTS ...`. The DropStmt.behavior
-- (CASCADE/RESTRICT) was ignored, so:
--   1. DROP TABLE tv_x CASCADE on a TVIEW with dependents raised
--      "cannot drop ... because other objects depend on it" internally, which the
--      catch_unwind swallowed into the opaque "PANIC ... Any { .. }" message.
--   2. A plain (RESTRICT) DROP on a TVIEW with dependents degraded to the same
--      opaque message instead of a clean, actionable error.

\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo '=========================================='
\echo 'Regression Test: DROP TABLE tv_* CASCADE'
\echo '=========================================='

CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Build a fully-converted TVIEW (metadata + supporting indexes) via the normal path.
DROP TABLE IF EXISTS tb_cascade_post CASCADE;
CREATE TABLE tb_cascade_post (
    id BIGINT PRIMARY KEY,
    uid UUID NOT NULL DEFAULT gen_random_uuid(),
    title TEXT
);
INSERT INTO tb_cascade_post (id, title) VALUES (1, 'a'), (2, 'b');

SELECT pg_tviews_create(
    'cascade_post',
    'SELECT id AS pk_cascade_post, uid AS id, jsonb_build_object(''title'', title) AS data FROM tb_cascade_post'
);

-- Create a dependent object on the materialized table. Dropping tv_cascade_post now
-- requires CASCADE; previously the internal non-CASCADE drop panicked.
CREATE VIEW dependent_report AS SELECT pk_cascade_post, data FROM tv_cascade_post;

\echo ''
\echo '### Test 1: DROP TABLE tv_* CASCADE succeeds and removes dependents'

DROP TABLE public.tv_cascade_post CASCADE;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_class WHERE relname = 'tv_cascade_post') THEN
        RAISE EXCEPTION 'tv_cascade_post still exists after CASCADE drop';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_class WHERE relname = 'dependent_report') THEN
        RAISE EXCEPTION 'dependent_report should have been dropped by CASCADE';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'cascade_post') THEN
        RAISE EXCEPTION 'pg_tview_meta row for cascade_post should be gone';
    END IF;
    RAISE NOTICE '✓ DROP TABLE tv_* CASCADE removed the TVIEW and its dependents';
END $$;

\echo ''
\echo '### Test 2: plain (RESTRICT) DROP with dependents gives a clean, actionable error'

-- Recreate the TVIEW and a dependent.
DROP TABLE IF EXISTS tb_cascade_post CASCADE;
CREATE TABLE tb_cascade_post (
    id BIGINT PRIMARY KEY,
    uid UUID NOT NULL DEFAULT gen_random_uuid(),
    title TEXT
);
SELECT pg_tviews_create(
    'cascade_post',
    'SELECT id AS pk_cascade_post, uid AS id, jsonb_build_object(''title'', title) AS data FROM tb_cascade_post'
);
CREATE VIEW dependent_report AS SELECT pk_cascade_post, data FROM tv_cascade_post;

DO $$
DECLARE
    err_msg TEXT;
BEGIN
    BEGIN
        DROP TABLE public.tv_cascade_post;  -- no CASCADE
        RAISE EXCEPTION 'expected RESTRICT drop to fail because of dependent_report';
    EXCEPTION WHEN dependent_objects_still_exist THEN
        -- Correct, actionable PostgreSQL error (SQLSTATE 2BP01), not the opaque
        -- "PANIC ... Any { .. }" message.
        GET STACKED DIAGNOSTICS err_msg = MESSAGE_TEXT;
        IF err_msg LIKE '%Any { .. }%' THEN
            RAISE EXCEPTION 'got the degraded panic message instead of a clean error: %', err_msg;
        END IF;
        RAISE NOTICE '✓ RESTRICT drop reported a clean dependency error: %', err_msg;
    END;
END $$;

-- Clean up
DROP TABLE public.tv_cascade_post CASCADE;
DROP TABLE IF EXISTS tb_cascade_post CASCADE;

\echo '=========================================='
\echo 'Regression test #47 completed'
\echo '=========================================='
