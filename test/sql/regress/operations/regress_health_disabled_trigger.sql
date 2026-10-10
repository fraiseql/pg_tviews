-- The health check reports a disabled pg_tviews trigger as an error.
--
-- A disabled row trigger leaves its TVIEW stale with no error at all, and a
-- disabled flush trigger fails every commit that writes its table (55000), whose
-- hint sends the user to the health check: it must name the trigger.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_health_disabled_trigger.sql
-- expect-output: health_disabled_trigger: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
SELECT pg_tviews_create('tv_user', $$ SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);

DO $$
DECLARE t name := (SELECT tgname FROM pg_trigger
                   WHERE tgrelid = 'tb_user'::regclass AND tgname LIKE 'trg_tview_row_%');
BEGIN
    EXECUTE format('ALTER TABLE tb_user DISABLE TRIGGER %I', t);
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check()
                   WHERE component = 'triggers' AND status = 'ERROR'
                     AND message LIKE '%disabled%' AND message LIKE '%' || t || '%') THEN
        RAISE EXCEPTION 'FAIL: the health check does not report the disabled trigger: %',
            (SELECT string_agg(status || ' ' || component || ': ' || message, '; ')
               FROM tviews.pg_tviews_health_check());
    END IF;
    EXECUTE format('ALTER TABLE tb_user ENABLE TRIGGER %I', t);
    IF EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check()
               WHERE component = 'triggers' AND status <> 'OK') THEN
        RAISE EXCEPTION 'FAIL: the health check still reports the re-enabled trigger';
    END IF;
END $$;

\echo 'health_disabled_trigger: PASS'
