-- Issue #283: pg_tviews' refresh statements never JIT-compile.
--
-- A refresh is OLTP-shaped: a few rows by key, or a TVIEW rebuilt once. JIT
-- compiled them when their estimated cost crossed jit_above_cost (a wide view, a
-- cold table's statistics), adding tens to hundreds of milliseconds per statement
-- for nothing. Every refresh statement runs with jit = off, as it does with
-- plan_cache_mode pinned, whatever the session or the server says; the session's
-- own setting is untouched. A function of the view reports the setting it runs
-- under, so the test needs no JIT-capable build.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_refresh_jit_off.sql
-- expect-output: jit in refresh: off
-- reject-output: jit in refresh: on
-- expect-output: refresh jit off: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#283 FAIL: %', what; END IF; END $$;

-- Reports the jit setting each evaluation runs under.
CREATE FUNCTION jit_seen(v text) RETURNS text LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    RAISE WARNING 'jit in refresh: %', pg_catalog.current_setting('jit');
    RETURN v;
END $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'a');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('name', jit_seen(i.name)) AS data
    FROM tb_item i $$);

SET jit = on;
SET jit_above_cost = 0;
-- A keyed refresh, and a full one.
INSERT INTO tb_item (pk_item, name) VALUES (2, 'b');
SELECT tviews.pg_tviews_refresh('item');
SELECT must(current_setting('jit') = 'on', 'the session''s jit changed');

DROP EXTENSION pg_tviews CASCADE;
\echo 'refresh jit off: PASS'
