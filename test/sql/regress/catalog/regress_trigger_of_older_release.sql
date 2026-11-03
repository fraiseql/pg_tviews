-- A pg_tviews trigger of an older release fails the write, and re-registration
-- replaces it.
--
-- A table carries one set of pg_tviews triggers, none with an argument. Older
-- releases installed one set per TVIEW, each trigger naming its TVIEW as its
-- argument. One left behind (restored from an old dump, created by hand) would
-- serve every TVIEW of the table again, beside the table's own set: it raises,
-- naming the remedy, instead. The health check reports it as an error, and
-- pg_tviews_reregister_all() drops it and restores the table's set.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_trigger_of_older_release.sql
-- expect-output: trigger of an older release: PASS

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
    RAISE EXCEPTION 'FAIL: % succeeded through a trigger of an older release', label;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT;
    IF msg LIKE 'FAIL:%' THEN
        RAISE;
    END IF;
    IF msg NOT LIKE '%older release%' OR msg NOT LIKE '%pg_tviews_reregister_all%' THEN
        RAISE EXCEPTION 'FAIL: % failed for another reason: %', label, msg;
    END IF;
END $$;

CREATE FUNCTION ours() RETURNS text LANGUAGE sql AS $$
    SELECT string_agg(tgname || '/' || tgnargs, ',' ORDER BY tgname) FROM pg_trigger
    WHERE tgrelid = 'tb_user'::regclass AND tgname LIKE 'trg_tview_%' $$;

-- Each kind of handler refuses to run as a per-TVIEW trigger.
CREATE TRIGGER trg_tview_row_user_on_public_tb_user AFTER UPDATE ON tb_user
    FOR EACH ROW EXECUTE FUNCTION tviews.pg_tview_trigger_handler("user");
SELECT must_fail('UPDATE tb_user SET name = ''grace''', 'a row trigger');
DROP TRIGGER trg_tview_row_user_on_public_tb_user ON tb_user;

CREATE TRIGGER trg_tview_delta_i_user_on_public_tb_user AFTER INSERT ON tb_user
    REFERENCING NEW TABLE AS pg_tviews_new
    FOR EACH STATEMENT EXECUTE FUNCTION tviews.pg_tview_delta_trigger("user");
SELECT must_fail('INSERT INTO tb_user (pk_user, name) VALUES (2, ''alan'')', 'a statement trigger');
DROP TRIGGER trg_tview_delta_i_user_on_public_tb_user ON tb_user;

CREATE TRIGGER trg_tview_flush_user_on_public_tb_user AFTER DELETE ON tb_user
    FOR EACH STATEMENT EXECUTE FUNCTION tviews.pg_tview_flush_trigger("user");
SELECT must_fail('DELETE FROM tb_user WHERE pk_user = 0', 'a flush trigger');
DROP TRIGGER trg_tview_flush_user_on_public_tb_user ON tb_user;

CREATE TRIGGER trg_tview_truncate_user_on_public_tb_user AFTER TRUNCATE ON tb_user
    FOR EACH STATEMENT EXECUTE FUNCTION tviews.pg_tview_truncate_trigger("user");
SELECT must_fail('TRUNCATE tb_user', 'a truncate trigger');

-- The health check names it as an error.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_check_health()
                   WHERE component = 'triggers' AND status = 'ERROR'
                     AND message LIKE '%older release%trg_tview_truncate_user_on_public_tb_user%') THEN
        RAISE EXCEPTION 'FAIL: the health check does not report the trigger of an older release: %',
            (SELECT string_agg(status || ' ' || component || ': ' || message, '; ')
             FROM tviews.pg_tviews_check_health());
    END IF;
END $$;

-- Re-registration replaces it with the table's own set.
SELECT count(*) FROM tviews.pg_tviews_reregister_all();
DO $$
BEGIN
    IF ours() <> 'trg_tview_flush_on_public_tb_user/0,trg_tview_row_on_public_tb_user/0,'
                 'trg_tview_truncate_on_public_tb_user/0' THEN
        RAISE EXCEPTION 'FAIL: re-registration left %', ours();
    END IF;
END $$;

UPDATE tb_user SET name = 'grace';
TRUNCATE tb_user;
INSERT INTO tb_user (pk_user, name) VALUES (1, 'grace');
DO $$
BEGIN
    IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) IS DISTINCT FROM 'grace' THEN
        RAISE EXCEPTION 'FAIL: tv_user stale once the trigger is replaced';
    END IF;
END $$;

\echo 'trigger of an older release: PASS'
