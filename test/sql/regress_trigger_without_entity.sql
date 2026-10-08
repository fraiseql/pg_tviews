-- A pg_tviews trigger that names no TVIEW fails the write.
--
-- Every trigger pg_tviews installs carries the entity it serves as its
-- argument; the upgrade drops the ones older releases installed without it. A
-- trigger left without one (created by hand, restored from an old dump) cannot
-- tell which TVIEW to refresh: it raises, naming the remedy, instead of doing
-- nothing.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_trigger_without_entity.sql
-- expect-output: trigger without entity: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$$);

CREATE FUNCTION must_fail(sql text, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    msg text;
BEGIN
    EXECUTE sql;
    RAISE EXCEPTION 'FAIL: % succeeded through a trigger that names no TVIEW', label;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT;
    IF msg LIKE 'FAIL:%' THEN
        RAISE;
    END IF;
    IF msg NOT LIKE '%names no TVIEW%' OR msg NOT LIKE '%pg_tviews_reregister_all%' THEN
        RAISE EXCEPTION 'FAIL: % failed for another reason: %', label, msg;
    END IF;
END $$;

CREATE TRIGGER untagged_row AFTER UPDATE ON tb_user
    FOR EACH ROW EXECUTE FUNCTION tviews.pg_tview_trigger_handler();
SELECT must_fail('UPDATE tb_user SET name = ''grace''', 'a row trigger');
DROP TRIGGER untagged_row ON tb_user;

CREATE TRIGGER untagged_delta AFTER INSERT ON tb_user REFERENCING NEW TABLE AS pg_tviews_new
    FOR EACH STATEMENT EXECUTE FUNCTION tviews.pg_tview_delta_trigger();
SELECT must_fail('INSERT INTO tb_user (pk_user, name) VALUES (2, ''alan'')', 'a statement trigger');
DROP TRIGGER untagged_delta ON tb_user;

CREATE TRIGGER untagged_truncate AFTER TRUNCATE ON tb_user
    FOR EACH STATEMENT EXECUTE FUNCTION tviews.pg_tview_truncate_trigger();
SELECT must_fail('TRUNCATE tb_user', 'a truncate trigger');
DROP TRIGGER untagged_truncate ON tb_user;

UPDATE tb_user SET name = 'grace';
DO $$
BEGIN
    IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) <> 'grace' THEN
        RAISE EXCEPTION 'FAIL: tv_user stale once the triggers are gone';
    END IF;
END $$;

\echo 'trigger without entity: PASS'
