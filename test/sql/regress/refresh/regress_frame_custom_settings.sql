-- E1-01: a writer's custom settings do not reach what the refresh renders. Every
-- frame resets each setting with a dot in its name (placeholders and extensions'
-- settings, pg_tviews.* aside) that the session, a SET LOCAL, a function's SET, a
-- connection option or the writer's role gave, to the value of the server, the
-- database and ALTER ROLE ALL; a placeholder with none of those reads as unset
-- (NULL, or '' once the backend has seen the name: PostgreSQL keeps a placeholder
-- it has seen). A function of the view and the definition itself both read the
-- settings each evaluation runs under; the session's own settings are untouched.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_frame_custom_settings.sql
-- expect-output: frame_custom_settings: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_fcs');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'frame_custom_settings FAIL: %', what; END IF; END $$;

-- The custom settings each evaluation runs under, '' read as unset.
CREATE FUNCTION custom_seen(v text) RETURNS jsonb LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    RETURN jsonb_build_object('v', v,
        'locale', nullif(pg_catalog.current_setting('app.locale', true), ''),
        'tenant', nullif(pg_catalog.current_setting('app.tenant', true), ''));
END $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'a');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id,
           custom_seen(i.name) || jsonb_build_object(
               'direct', nullif(pg_catalog.current_setting('app.locale', true), '')) AS data
    FROM tb_item i $$);
GRANT SELECT, UPDATE ON tb_item TO regress_fcs_writer;
GRANT SELECT ON tv_item TO regress_fcs_writer;
GRANT EXECUTE ON FUNCTION must(boolean, text) TO regress_fcs_writer;

CREATE FUNCTION unset(d jsonb) RETURNS boolean LANGUAGE sql IMMUTABLE AS $$
    SELECT d->>'locale' IS NULL AND d->>'tenant' IS NULL AND d->>'direct' IS NULL $$;
GRANT EXECUTE ON FUNCTION unset(jsonb) TO regress_fcs_writer;

-- 1. A session's SET does not reach the refresh, whoever writes.
SET app.locale = 'xx-SESSION';
SET app.tenant = '42';
UPDATE tb_item SET name = 'b' WHERE pk_item = 1;
SELECT must(unset(data), 'a session''s setting reached the refresh: ' || data::text) FROM tv_item;
SELECT must(current_setting('app.locale') = 'xx-SESSION', 'the session''s own setting changed');
SET ROLE regress_fcs_writer;
UPDATE tb_item SET name = 'c' WHERE pk_item = 1;
SELECT must(unset(data), 'a writer''s setting reached the refresh: ' || data::text) FROM tv_item;
RESET ROLE;
-- A full refresh too, and with the patch path off.
SELECT tviews.pg_tviews_refresh('item');
SELECT must(unset(data), 'a full refresh used the session''s setting: ' || data::text) FROM tv_item;
SET pg_tviews.direct_patch_enabled = off;
UPDATE tb_item SET name = 'd' WHERE pk_item = 1;
SELECT must(unset(data), 'a recompute used the session''s setting: ' || data::text) FROM tv_item;
RESET pg_tviews.direct_patch_enabled;
RESET app.locale;
RESET app.tenant;

-- 2. SET LOCAL, set_config() and a function's SET clause.
BEGIN;
SET LOCAL app.locale = 'xx-LOCAL';
UPDATE tb_item SET name = 'e' WHERE pk_item = 1;
SELECT must(unset(data), 'a SET LOCAL reached the refresh: ' || data::text) FROM tv_item;
COMMIT;
SELECT set_config('app.locale', 'xx-SET-CONFIG', false) IS NOT NULL AS set;
UPDATE tb_item SET name = 'f' WHERE pk_item = 1;
SELECT must(unset(data), 'set_config() reached the refresh: ' || data::text) FROM tv_item;
RESET app.locale;
CREATE FUNCTION write_as(n text) RETURNS void LANGUAGE sql
SET app.locale = 'xx-FUNCTION'
BEGIN ATOMIC
    UPDATE tb_item SET name = n WHERE pk_item = 1;
END;
SELECT write_as('g');
SELECT must(unset(data), 'a function''s SET reached the refresh: ' || data::text) FROM tv_item;

-- 3. A role's setting and a connection option are the writer's own; the
--    database's value is the server's and applies, under them too.
ALTER ROLE regress_fcs_writer SET app.tenant = '7';
\c - regress_fcs_writer
SET client_min_messages TO WARNING;
SELECT must(current_setting('app.tenant') = '7', 'the writer''s role setting is not in force');
UPDATE tb_item SET name = 'h' WHERE pk_item = 1;
SELECT must(unset(data), 'a role''s setting reached the refresh: ' || data::text) FROM tv_item;
SELECT format('dbname=%s user=regress_fcs_writer options=''-c app.locale=xx-CLIENT''',
              current_database()) AS client_conn \gset
\c :client_conn
SET client_min_messages TO WARNING;
SELECT must(current_setting('app.locale') = 'xx-CLIENT', 'the connection option is not in force');
UPDATE tb_item SET name = 'i' WHERE pk_item = 1;
SELECT must(unset(data), 'a connection option reached the refresh: ' || data::text) FROM tv_item;
\c - postgres
SET client_min_messages TO WARNING;
SELECT format('ALTER DATABASE %I SET app.locale = %L', current_database(), 'fr-FR') \gexec
\c - regress_fcs_writer
SET client_min_messages TO WARNING;
SET app.locale = 'xx-OVER-DB';
UPDATE tb_item SET name = 'j' WHERE pk_item = 1;
SELECT must(data->>'locale' = 'fr-FR' AND data->>'direct' = 'fr-FR' AND data->>'tenant' IS NULL,
            'the database''s setting did not apply, or a session''s or role''s did: ' || data::text)
FROM tv_item;
\c - postgres
SET client_min_messages TO WARNING;
SELECT format('ALTER DATABASE %I RESET app.locale', current_database()) \gexec
ALTER ROLE regress_fcs_writer RESET app.tenant;

SELECT 'frame_custom_settings: PASS' AS result;

DROP FUNCTION write_as(text);
DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_item;
DROP FUNCTION custom_seen(text), unset(jsonb);
SELECT regress_drop_roles('regress_fcs');
