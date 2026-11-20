-- The settings option pins custom settings in a TVIEW's owner frames (D4-C): its
-- fill, its refreshes and its recomputes read the pinned value, whatever a
-- session sets. The registry publishes it; replacing it recomputes the rows.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_frame_settings_option.sql
-- expect-output: tv_doc reads the setting app.locale: its refreshes read it as 'fr-FR'
-- expect-output: frame_settings_option: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'frame_settings_option FAIL: %', what; END IF; END $$;

CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text NOT NULL);
INSERT INTO tb_doc (pk_doc, name) VALUES (1, 'a'), (2, 'b');

-- 1. Pinned at creation: the fill, a recompute and a full refresh read it.
SET app.locale = 'xx-SESSION';
SET client_min_messages TO NOTICE;
SELECT tviews.pg_tviews_create_or_replace('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('name', d.name,
           'locale', nullif(current_setting('app.locale', true), '')) AS data
    FROM tb_doc d $$, '{"settings": {"app.locale": "fr-FR"}}') AS status;
SET client_min_messages TO WARNING;
SELECT must((SELECT bool_and(data->>'locale' = 'fr-FR') FROM tv_doc), 'the fill did not pin it');
SET pg_tviews.direct_patch_enabled = off;
UPDATE tb_doc SET name = 'a2' WHERE pk_doc = 1;
SELECT must((SELECT data->>'locale' FROM tv_doc WHERE pk_doc = 1) = 'fr-FR',
            'a recompute did not pin it');
RESET pg_tviews.direct_patch_enabled;
SELECT tviews.pg_tviews_refresh('doc');
SELECT must((SELECT bool_and(data->>'locale' = 'fr-FR') FROM tv_doc), 'a full refresh did not pin it');
SELECT must(current_setting('app.locale') = 'xx-SESSION', 'the session''s own setting changed');

-- 2. Published, and reported by the health check.
SELECT must((SELECT options->'settings' = '{"app.locale": "fr-FR"}' FROM tviews.registry
             WHERE entity = 'doc'), 'the registry does not publish it');
SELECT must((SELECT status = 'OK' AND message LIKE 'tv_doc reads app.locale as ''fr-FR''%'
             FROM tviews.pg_tviews_check_health() WHERE component = 'frame_settings'),
            'health does not report the pinned value');

-- 3. Replaced: the rows are recomputed under the new value; unchanged, nothing.
SELECT must(tviews.pg_tviews_create_or_replace('tv_doc', definition,
                '{"settings": {"app.locale": "de-DE"}}') = 'altered', 'a new value is no alter')
FROM tviews.pg_tview_meta WHERE entity = 'doc';
SELECT must((SELECT bool_and(data->>'locale' = 'de-DE') FROM tv_doc), 'the new value was not applied');
SELECT must(tviews.pg_tviews_create_or_replace('tv_doc', definition,
                '{"settings": {"app.locale": "de-DE"}}') = 'unchanged', 'the same value changed it')
FROM tviews.pg_tview_meta WHERE entity = 'doc';
SELECT must(tviews.pg_tviews_create_or_replace('tv_doc', definition, '{}') = 'altered',
            'dropping the option is no alter')
FROM tviews.pg_tview_meta WHERE entity = 'doc';
SELECT must((SELECT bool_and(data->>'locale' IS NULL) FROM tv_doc), 'the dropped pin still applies');

-- 4. Only custom settings, with string values.
DO $$
DECLARE options text;
BEGIN
    FOREACH options IN ARRAY ARRAY['{"settings": {"work_mem": "1MB"}}',
                                   '{"settings": {"pg_tviews.direct_patch_enabled": "off"}}',
                                   '{"settings": {"app.locale": 1}}'] LOOP
        BEGIN
            PERFORM tviews.pg_tviews_create_or_replace('tv_bad', 'SELECT 1', options::jsonb);
            RAISE EXCEPTION 'frame_settings_option FAIL: % was accepted', options;
        EXCEPTION WHEN invalid_parameter_value THEN NULL;
        END;
    END LOOP;
END $$;

RESET app.locale;
SELECT 'frame_settings_option: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_doc;
DROP FUNCTION must(boolean, text);
