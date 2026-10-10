-- pg_tviews_drop() records one DROP in the audit log, as DROP TABLE tv_x does.
--
-- The DROP TABLE it issues itself fires the sql_drop event trigger, which
-- deregisters (and audits) a TVIEW whose table went: the TVIEW is deregistered
-- before its relations are dropped, so the drop is recorded once.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_audit_drop_once.sql
-- expect-output: audit_drop_once: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), label text);
SELECT pg_tviews_create('tv_tag', $$
    SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM tb_tag $$);

SET pg_tviews.audit_enabled = on;
SELECT pg_tviews_drop('tv_tag');

DO $$
DECLARE got text;
BEGIN
    SELECT string_agg(operation || ':' || entity, ' ' ORDER BY log_id) INTO got
    FROM tviews.pg_tview_audit_log;
    IF got IS DISTINCT FROM 'DROP:tag' THEN
        RAISE EXCEPTION 'audit_drop_once FAIL: pg_tviews_drop audited %', got;
    END IF;
END $$;

\echo 'audit_drop_once: PASS'
