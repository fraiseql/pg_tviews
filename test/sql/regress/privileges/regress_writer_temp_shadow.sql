-- The functions a TVIEW's refresh runs, as its owner, resolve names only where
-- the owner put them: never in the writer's temporary schema.
--
-- PostgreSQL searches the session's temporary schema first for relations and
-- types unless a search_path lists pg_temp explicitly, later. A function the
-- definition calls that sets its own search_path must therefore list pg_temp
-- last: pg_tviews_create and pg_tviews_create_or_replace refuse one that does
-- not, and so do user triggers on the TVIEW's table.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_writer_temp_shadow.sql
-- expect-output: temp shadow: names resolve where the owner put them

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_ts');

-- The SQLSTATE and hint `stmt` fails with, NULL when it succeeds.
CREATE FUNCTION failure(stmt text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE
    hint text;
BEGIN
    EXECUTE stmt;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS hint = PG_EXCEPTION_HINT;
    RETURN SQLSTATE || ' ' || coalesce(hint, '');
END $$;
GRANT EXECUTE ON FUNCTION failure(text) TO PUBLIC;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'temp shadow: %', what; END IF; END $$;
GRANT EXECUTE ON FUNCTION must(boolean, text) TO PUBLIC;

CREATE SCHEMA app AUTHORIZATION regress_ts_owner_a;
GRANT USAGE ON SCHEMA app TO PUBLIC;
SET ROLE regress_ts_owner_a;
CREATE TABLE app.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          body text);
INSERT INTO app.tb_note (pk_note, body) VALUES (1, 'n1');
CREATE FUNCTION app.label_bare(t text) RETURNS text LANGUAGE sql IMMUTABLE
    SET search_path = app AS $$ SELECT upper(t) $$;
CREATE FUNCTION app.label_first(t text) RETURNS text LANGUAGE sql IMMUTABLE
    SET search_path = pg_temp, app AS $$ SELECT upper(t) $$;
CREATE FUNCTION app.label_quoted(t text) RETURNS text LANGUAGE sql IMMUTABLE
    SET search_path = "app", "PG_TEMP" AS $$ SELECT upper(t) $$;
CREATE FUNCTION app.label(t text) RETURNS text LANGUAGE sql IMMUTABLE
    SET search_path = app, pg_temp AS $$ SELECT upper(t) $$;

-- Part A: a search_path without pg_temp last is refused, with the fix.
SELECT must(failure(format($f$SELECT tviews.pg_tviews_create('app.tv_note', %L)$f$, $q$
    SELECT n.pk_note, n.id, jsonb_build_object('id', n.id, 'body', app.label_bare(n.body)) AS data
    FROM app.tb_note n $q$)) LIKE '42P13 %SET search_path = app, pg_temp%',
            'a path without pg_temp accepted');
SELECT must(failure(format($f$SELECT tviews.pg_tviews_create('app.tv_note', %L)$f$, $q$
    SELECT n.pk_note, n.id, jsonb_build_object('id', n.id, 'body', app.label_first(n.body)) AS data
    FROM app.tb_note n $q$)) LIKE '42P13 %',
            'a path with pg_temp first accepted');
SELECT must(failure(format($f$SELECT tviews.pg_tviews_create('app.tv_note', %L)$f$, $q$
    SELECT n.pk_note, n.id, jsonb_build_object('id', n.id, 'body', app.label_quoted(n.body)) AS data
    FROM app.tb_note n $q$)) LIKE '42P13 %',
            'a quoted "PG_TEMP", another schema, accepted');
SELECT tviews.pg_tviews_create('app.tv_note', $q$
    SELECT n.pk_note, n.id, jsonb_build_object('id', n.id, 'body', app.label(n.body)) AS data
    FROM app.tb_note n $q$);

-- A trigger on the TVIEW's table runs inside the owner's refresh too.
CREATE FUNCTION app.noted() RETURNS trigger LANGUAGE plpgsql SET search_path = app AS $$
BEGIN RETURN NEW; END $$;
CREATE TRIGGER noted BEFORE UPDATE ON app.tv_note FOR EACH ROW EXECUTE FUNCTION app.noted();
SELECT must(failure($f$SELECT tviews.pg_tviews_create_or_replace('app.tv_note',
    (SELECT definition FROM tviews.pg_tview_meta WHERE entity = 'note'),
    options => '{"uncascaded_policy": "warn"}')$f$) LIKE '42P13 %SET search_path = app, pg_temp%',
            'a trigger with a path without pg_temp accepted');
ALTER FUNCTION app.noted() SET search_path = app, pg_temp;
SELECT tviews.pg_tviews_create_or_replace('app.tv_note',
    (SELECT definition FROM tviews.pg_tview_meta WHERE entity = 'note'),
    options => '{"uncascaded_policy": "warn"}');

-- Part B: what no check of the definition sees. f() runs g() through EXECUTE;
-- g() sets a search_path without pg_temp. A writer's temporary relation or type
-- named like one the owner resolves makes the write fail (42501, at the end of
-- the statement, when its refresh runs), before the owner's code runs; other
-- temporary objects do not.
CREATE TABLE app.tb_tag (pk_tag int PRIMARY KEY);
CREATE FUNCTION app.g(k int) RETURNS int LANGUAGE plpgsql SET search_path = app AS $$
BEGIN RETURN (SELECT count(*)::int4 FROM tb_tag WHERE pk_tag = k); END $$;
CREATE FUNCTION app.f(k int) RETURNS int LANGUAGE plpgsql IMMUTABLE
    SET search_path = app, pg_temp AS $$
DECLARE
    n int;
BEGIN EXECUTE 'SELECT app.g($1)' INTO n USING k; RETURN n; END $$;
CREATE TABLE app.tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
SELECT tviews.pg_tviews_create('app.tv_item', $q$
    SELECT i.pk_item, i.id, jsonb_build_object('id', i.id, 'tags', app.f(i.pk_item)) AS data
    FROM app.tb_item i $q$);
GRANT SELECT, INSERT, UPDATE ON app.tb_item TO regress_ts_writer;
RESET ROLE;
TRUNCATE public.who_log;

SET ROLE regress_ts_writer;
CREATE TEMP VIEW tb_tag AS SELECT 1 AS pk_tag WHERE public.who('shadow') = 1;
\set ON_ERROR_STOP off
INSERT INTO app.tb_item (pk_item, name) VALUES (1, 'a');
\set ON_ERROR_STOP on
SELECT must(:'SQLSTATE' = '42501', 'a temporary relation named like the owner''s was used: ' || :'SQLSTATE');
DROP VIEW pg_temp.tb_tag;
-- A temporary table named like a type: its row type would be the type.
CREATE TEMP TABLE int4 (x int);
\set ON_ERROR_STOP off
INSERT INTO app.tb_item (pk_item, name) VALUES (1, 'a');
\set ON_ERROR_STOP on
SELECT must(:'SQLSTATE' = '42501', 'a temporary table named like a type was allowed: ' || :'SQLSTATE');
DROP TABLE pg_temp.int4;
-- One named like a catalog relation.
CREATE TEMP TABLE pg_class (x int);
\set ON_ERROR_STOP off
INSERT INTO app.tb_item (pk_item, name) VALUES (1, 'a');
\set ON_ERROR_STOP on
SELECT must(:'SQLSTATE' = '42501', 'a temporary table named like a catalog relation was allowed: ' || :'SQLSTATE');
DROP TABLE pg_temp.pg_class;
-- Temporary objects of other names, and dropped ones, change nothing.
CREATE TEMP TABLE staging_import (x int);
INSERT INTO app.tb_item (pk_item, name) VALUES (1, 'a');
DROP TABLE staging_import;
UPDATE app.tb_item SET name = 'b' WHERE pk_item = 1;
RESET ROLE;
SELECT must(NOT EXISTS (SELECT 1 FROM public.who_log WHERE step = 'shadow'),
            'the owner''s code read a temporary object of the writer''s');
SELECT must((SELECT data->>'tags' FROM app.tv_item WHERE pk_item = 1) = '0',
            'the write next to other temporary objects was not refreshed');

SELECT 'temp shadow: names resolve where the owner put them' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP SCHEMA app CASCADE;
SELECT regress_drop_roles('regress_ts');
