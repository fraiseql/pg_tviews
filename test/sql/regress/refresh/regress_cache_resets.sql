-- The caches count the full relcache invalidations they received (test-hooks builds).
--
-- A backend that falls too far behind the shared invalidation queue, which DDL in
-- any database of the cluster fills, is sent a reset: every relation counts as
-- changed, and the caches are dropped. Its next write then reads the catalog again,
-- which tests counting catalog lookups must tell apart from a regression.
-- pg_tviews_show_queue()'s `cache_resets` counts them, in test-hooks builds only.
-- CREATE PUBLICATION … FOR ALL TABLES sends the same full invalidation.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_cache_resets.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'cache resets FAIL: %', what; END IF; END $$;

SELECT tviews.pg_tviews_show_queue() ? 'cache_resets' AS hooks \gset
\if :hooks
\else
\echo 'SKIP: cache_resets needs a test-hooks build'
\quit
\endif

SELECT (tviews.pg_tviews_show_queue()->>'cache_resets')::int AS before \gset
CREATE TABLE plain_t (a int);
SELECT must((tviews.pg_tviews_show_queue()->>'cache_resets')::int = :before,
            'a relation''s invalidation counted as a reset');
CREATE PUBLICATION regress_cache_resets FOR ALL TABLES;
DROP PUBLICATION regress_cache_resets;
SELECT must((tviews.pg_tviews_show_queue()->>'cache_resets')::int > :before,
            'a full relcache invalidation not counted');

DROP EXTENSION pg_tviews CASCADE;
\echo 'cache resets: PASS'
