-- The monitoring surface reports only real data. pg_tviews_performance_stats()
-- works where query_to_xml/xpath are unusable (a server built without libxml);
-- the placeholder views and pg_tviews_hook_status() are gone; an unknown
-- pg_tviews.* setting is refused.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_monitoring_surface.sql
--
-- expect-output: monitoring surface: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_post (pk_post, title) VALUES (1, 'a'), (2, 'b');
SELECT pg_tviews_create('tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post $$);

-- ── item 3: performance_stats without the XML functions ─────────────────────
-- Here as on a server built without libxml, query_to_xml and xpath are unusable.
REVOKE EXECUTE ON FUNCTION pg_catalog.query_to_xml(text, boolean, boolean, text),
                           pg_catalog.xpath(text, xml), pg_catalog.xpath(text, xml, text[])
    FROM PUBLIC;
DROP ROLE IF EXISTS monitoring_reader;
CREATE ROLE monitoring_reader;
GRANT USAGE ON SCHEMA tviews TO monitoring_reader;
GRANT SELECT ON tv_post TO monitoring_reader;
SET ROLE monitoring_reader;
DO $$
DECLARE n bigint;
BEGIN
    SELECT row_count INTO n FROM tviews.pg_tviews_performance_stats() WHERE entity = 'post';
    IF n IS DISTINCT FROM 2 THEN
        RAISE EXCEPTION 'item 3 FAIL: performance_stats row_count for post is %', n;
    END IF;
EXCEPTION WHEN insufficient_privilege OR feature_not_supported THEN
    RAISE EXCEPTION 'item 3 FAIL: performance_stats needs the XML functions: %', SQLERRM;
END $$;
RESET ROLE;
GRANT EXECUTE ON FUNCTION pg_catalog.query_to_xml(text, boolean, boolean, text),
                          pg_catalog.xpath(text, xml), pg_catalog.xpath(text, xml, text[])
    TO PUBLIC;
REVOKE ALL ON tv_post FROM monitoring_reader;
REVOKE USAGE ON SCHEMA tviews FROM monitoring_reader;
DROP ROLE monitoring_reader;

-- ── item 4 / N6: no placeholder objects ─────────────────────────────────────
DO $$ BEGIN
    IF to_regclass('tviews.pg_tviews_queue_realtime') IS NOT NULL
       OR to_regclass('tviews.pg_tviews_cache_stats') IS NOT NULL
       OR to_regclass('tviews.pg_tviews_performance_summary') IS NOT NULL
       OR to_regprocedure('tviews.pg_tviews_hook_status()') IS NOT NULL THEN
        RAISE EXCEPTION 'item 4 FAIL: placeholder monitoring objects still installed';
    END IF;
END $$;

-- ── D6: an unknown pg_tviews.* setting is refused ───────────────────────────
DO $$ BEGIN
    BEGIN
        SET pg_tviews.lock_timeout_ms = 1;
    EXCEPTION WHEN OTHERS THEN
        RETURN;
    END;
    RAISE EXCEPTION 'D6 FAIL: SET pg_tviews.lock_timeout_ms was accepted';
END $$;
-- Every real one still sets.
DO $$
DECLARE g record;
BEGIN
    FOR g IN SELECT name, setting FROM pg_settings WHERE name LIKE 'pg_tviews.%'
             AND context IN ('user', 'superuser') LOOP
        EXECUTE format('SET %s = %L', g.name, g.setting);
    END LOOP;
END $$;

\echo 'monitoring surface: PASS'
