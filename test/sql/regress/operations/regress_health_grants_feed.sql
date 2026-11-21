-- Regression test (E1-05, E1-15): two grants pg_tviews does not need, reported by
-- pg_tviews_check_health().
--   1. tv_writers: a role other than a TVIEW table's owner holding INSERT, UPDATE,
--      DELETE or TRUNCATE on it. pg_tviews is the table's only writer (its guard
--      refuses every other write); a grant of writes is a mistake waiting for the
--      guard to be disabled.
--   2. change_feed: with pg_tviews.publish_changes = rows, every role that can
--      LISTEN receives the keys of every TVIEW's changed rows: a login role that may
--      connect but not read a TVIEW is named.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_health_grants_feed.sql
-- expect-output: health_grants_feed: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'health_grants_feed FAIL: %', what; END IF; END $$;
CREATE FUNCTION health(c text) RETURNS text LANGUAGE sql AS $$
    SELECT status || ': ' || message FROM tviews.pg_tviews_check_health() WHERE component = c $$;

CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_post VALUES (1, DEFAULT, 'one');
CREATE TABLE tv_post AS SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;

DROP ROLE IF EXISTS regress_hgf_app;
DROP ROLE IF EXISTS regress_hgf_listener;
CREATE ROLE regress_hgf_app;
CREATE ROLE regress_hgf_listener LOGIN;

-- 1. Writes granted on a TVIEW table.
SELECT must(health('tv_writers') LIKE 'OK:%', 'no grant: ' || coalesce(health('tv_writers'), 'no row'));
GRANT SELECT, INSERT, UPDATE ON tv_post TO regress_hgf_app;
GRANT TRUNCATE ON tv_post TO PUBLIC;
SELECT must(health('tv_writers') LIKE 'WARNING:%tv_post%regress_hgf_app%INSERT%'
            AND health('tv_writers') LIKE '%PUBLIC%TRUNCATE%',
            'writes granted: ' || coalesce(health('tv_writers'), 'no row'));
REVOKE INSERT, UPDATE ON tv_post FROM regress_hgf_app;
REVOKE TRUNCATE ON tv_post FROM PUBLIC;
SELECT must(health('tv_writers') LIKE 'OK:%', 'SELECT alone: ' || health('tv_writers'));

-- 2. The change feed. Only the test's role may connect to this database: the
--    cluster's other login roles stay out of the message.
REVOKE CONNECT ON DATABASE :"DBNAME" FROM PUBLIC;
GRANT CONNECT ON DATABASE :"DBNAME" TO regress_hgf_listener;
SELECT must(health('change_feed') LIKE 'OK:%off%', 'feed off: ' || coalesce(health('change_feed'), 'no row'));
SET pg_tviews.publish_changes = rows;
SELECT must(health('change_feed') LIKE 'WARNING:%regress_hgf_listener (public.tv_post)%',
            'feed on, a listener without SELECT: ' || coalesce(health('change_feed'), 'no row'));
GRANT SELECT ON tv_post TO regress_hgf_listener;
SELECT must(health('change_feed') LIKE 'OK:%',
            'a listener with SELECT is named: ' || health('change_feed'));
REVOKE SELECT ON tv_post FROM regress_hgf_listener;
REVOKE CONNECT ON DATABASE :"DBNAME" FROM regress_hgf_listener;
SELECT must(health('change_feed') LIKE 'OK:%',
            'a role that may not connect is named: ' || health('change_feed'));
SET pg_tviews.publish_changes = types;
SELECT must(health('change_feed') LIKE 'OK:%type names%', 'types: ' || health('change_feed'));
RESET pg_tviews.publish_changes;
GRANT CONNECT ON DATABASE :"DBNAME" TO PUBLIC;

REVOKE ALL ON tv_post FROM regress_hgf_app;
DROP ROLE regress_hgf_app;
DROP ROLE regress_hgf_listener;

\echo 'health_grants_feed: PASS'
