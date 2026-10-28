-- pg_tviews' trigger functions attached by hand the wrong way fail with SQLSTATE
-- 39P01 (trigger protocol violated), as PostgreSQL's own trigger functions do,
-- not with an internal error.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_trigger_misuse.sql
--
-- expect-output: trigger misuse: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION sqlstate_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE;
END $$;
CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF ok IS NOT TRUE THEN RAISE EXCEPTION 'trigger misuse FAIL: %', msg; END IF;
END $$;

CREATE TABLE tb_thing (pk_thing bigint PRIMARY KEY, name text);
INSERT INTO tb_thing VALUES (1, 'a');

-- the row trigger handler as a statement trigger: no row
CREATE TRIGGER as_statement AFTER UPDATE ON tb_thing
    FOR EACH STATEMENT EXECUTE FUNCTION tviews.pg_tview_trigger_handler('thing');
SELECT must(sqlstate_of('UPDATE tb_thing SET name = ''b''') = '39P01',
            'a row trigger fired per statement: '
            || coalesce(sqlstate_of('UPDATE tb_thing SET name = ''b'''), 'no error'));
DROP TRIGGER as_statement ON tb_thing;

-- a trigger naming no TVIEW
CREATE TRIGGER no_entity AFTER UPDATE ON tb_thing
    FOR EACH ROW EXECUTE FUNCTION tviews.pg_tview_trigger_handler();
SELECT must(sqlstate_of('UPDATE tb_thing SET name = ''c''') = '39P01',
            'a trigger naming no TVIEW: '
            || coalesce(sqlstate_of('UPDATE tb_thing SET name = ''c'''), 'no error'));
DROP TRIGGER no_entity ON tb_thing;

\echo 'trigger misuse: PASS'
