-- The custom settings a definition reads with current_setting() (dialect.md,
-- Axis D). Every refresh reads them in the owner frame, where a session's value
-- never reaches. A setting read for a value is reported at registration with
-- the value the refreshes read, and by the health check's frame_settings. A
-- setting that decides which rows the TVIEW holds (WHERE, ON, HAVING, LIMIT, a
-- FROM function's arguments, of the definition or of a view or subquery it reads
-- in FROM) is refused: the TVIEW would hold the rows of the server's value only.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_setting_reads.sql
-- expect-output: tv_label reads the setting app.locale: its refreshes read it as unset
-- expect-output: tv_sub reads the setting app.locale: its refreshes read it as unset
-- expect-output: setting_reads: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'setting_reads FAIL: %', what; END IF; END $$;

-- Refused with 0A000 and a message naming the setting.
CREATE FUNCTION refused(entity text, def text, setting text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM tviews.pg_tviews_create(entity, def);
    RAISE EXCEPTION 'setting_reads FAIL: % was accepted', entity;
EXCEPTION WHEN feature_not_supported THEN
    IF SQLERRM NOT LIKE '%decides which rows it holds on ' || setting || '%' THEN
        RAISE EXCEPTION 'setting_reads FAIL: % refused for another reason: %', entity, SQLERRM;
    END IF;
END $$;

CREATE FUNCTION frame_settings() RETURNS text LANGUAGE sql AS $$
    SELECT status || ': ' || message FROM tviews.pg_tviews_check_health()
    WHERE component = 'frame_settings' $$;

CREATE TABLE tb_label (pk_label int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       tenant int NOT NULL, name text NOT NULL);
INSERT INTO tb_label (pk_label, tenant, name) VALUES (1, 1, 'a'), (2, 2, 'b');

SELECT must(frame_settings() LIKE 'OK: No TVIEW reads a custom setting%', 'no TVIEW: ' || frame_settings());

-- 1. Read for a value: accepted, reported.
SET client_min_messages TO NOTICE;
SELECT tviews.pg_tviews_create('tv_label', $$
    SELECT l.pk_label, l.id, jsonb_build_object('name', l.name,
           'locale', coalesce(nullif(current_setting('app.locale', true), ''), 'fr-FR')) AS data
    FROM tb_label l $$) IS NOT NULL AS created;
-- A subquery computing a value: its filter decides that value, not the rows.
SELECT tviews.pg_tviews_create('tv_sub', $$
    SELECT l.pk_label AS pk_sub, l.id, jsonb_build_object('peers',
           (SELECT count(*) FROM tb_label p
            WHERE p.tenant = l.tenant AND p.name = current_setting('app.locale', true))) AS data
    FROM tb_label l $$) IS NOT NULL AS created;
SET client_min_messages TO WARNING;
SELECT must((SELECT plan->'settings_read' = '["app.locale"]' FROM tviews.pg_tview_meta
             WHERE entity = 'label'), 'the plan records the setting read');
SELECT must(frame_settings() LIKE 'WARNING: tv_label reads app.locale, tv_sub reads app.locale, unset%',
            'health: ' || frame_settings());

-- With the database's value, the refreshes read it, and health says so.
SELECT format('ALTER DATABASE %I SET app.locale = %L', current_database(), 'de-DE') \gexec
\c
SET client_min_messages TO WARNING;
SELECT must(frame_settings() LIKE 'OK: tv_label reads app.locale as ''de-DE''%', 'health: ' || frame_settings());
-- A change of the server's value is no write: a full refresh applies it.
SET app.locale = 'xx-SESSION';
SELECT tviews.pg_tviews_refresh('label');
SELECT must((SELECT data->>'locale' FROM tv_label WHERE pk_label = 1) = 'de-DE',
            'the refresh did not read the database''s value');
SET pg_tviews.direct_patch_enabled = off;
UPDATE tb_label SET name = 'a2' WHERE pk_label = 1;
SELECT must((SELECT data->>'locale' FROM tv_label WHERE pk_label = 1) = 'de-DE',
            'the recompute did not read the database''s value');
RESET pg_tviews.direct_patch_enabled;
SELECT format('ALTER DATABASE %I RESET app.locale', current_database()) \gexec
\c
SET client_min_messages TO WARNING;

-- 2. Deciding the rows: refused, wherever the filter is.
SELECT refused('tv_where', $$ SELECT l.pk_label AS pk_where, l.id, jsonb_build_object('n', l.name) AS data
    FROM tb_label l WHERE l.tenant = current_setting('app.tenant')::int $$, 'the setting app.tenant');
SELECT refused('tv_on', $$ SELECT l.pk_label AS pk_on, l.id, jsonb_build_object('n', l.name) AS data
    FROM tb_label l JOIN tb_label o ON o.pk_label = l.pk_label
                    AND o.tenant = current_setting('app.tenant', true)::int $$, 'the setting app.tenant');
SELECT refused('tv_limit', $$ SELECT l.pk_label AS pk_limit, l.id, jsonb_build_object('n', l.name) AS data
    FROM tb_label l ORDER BY l.pk_label LIMIT current_setting('app.n')::int $$, 'the setting app.n');
SELECT refused('tv_exists', $$ SELECT l.pk_label AS pk_exists, l.id, jsonb_build_object('n', l.name) AS data
    FROM tb_label l WHERE EXISTS (SELECT 1 FROM tb_label o
                                  WHERE o.pk_label = l.pk_label
                                    AND o.tenant::text = current_setting('app.tenant')) $$,
               'the setting app.tenant');
SELECT refused('tv_computed', $$ SELECT l.pk_label AS pk_computed, l.id, jsonb_build_object('n', l.name) AS data
    FROM tb_label l WHERE l.name = current_setting('app.' || l.name, true) $$,
               'a setting it names by an expression');
CREATE VIEW v_mine AS SELECT * FROM tb_label WHERE tenant = current_setting('app.tenant')::int;
SELECT refused('tv_view', $$ SELECT m.pk_label AS pk_view, m.id, jsonb_build_object('n', m.name) AS data
    FROM v_mine m $$, 'the setting app.tenant');
SELECT refused('tv_derived', $$ SELECT d.pk_label AS pk_derived, d.id, jsonb_build_object('n', d.name) AS data
    FROM (SELECT * FROM tb_label GROUP BY pk_label
          HAVING max(tenant) = current_setting('app.tenant')::int) d $$, 'the setting app.tenant');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity NOT IN ('label', 'sub')),
            'a refused TVIEW was registered');

SELECT 'setting_reads: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP VIEW v_mine;
DROP TABLE tb_label;
DROP FUNCTION must(boolean, text), refused(text, text, text), frame_settings();
