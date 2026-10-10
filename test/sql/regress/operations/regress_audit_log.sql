-- pg_tviews.audit_enabled records TVIEW lifecycle and refresh events in
-- tviews.pg_tview_audit_log, and only what committed.
--
-- Entries are buffered during a transaction and written in one go. With the
-- setting off nothing is written, and entries buffered while it was off do not
-- leak into a later audited transaction. With it on: one CREATE (with the
-- definition) per pg_tviews_create, one REFRESH per refreshed entity per
-- statement (rows_affected = rows refreshed), one DROP per DROP TABLE tv_x. A
-- rolled-back transaction or savepoint leaves no entry.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_audit_log.sql
-- expect-output: audit_log: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'audit_log FAIL: %', what; END IF; END $$;
-- The log as `operation:entity:rows_affected`, in insertion order.
CREATE FUNCTION audit() RETURNS text LANGUAGE sql AS $$
    SELECT coalesce(string_agg(operation || ':' || entity || ':' || coalesce(rows_affected::text, '-'),
                               ' ' ORDER BY log_id), '')
    FROM tviews.pg_tview_audit_log $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'a'), (2, 'b'), (3, 'c');

-- 1. Off: nothing is written.
SET pg_tviews.audit_enabled = off;
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
UPDATE tb_user SET name = name || '0';
SELECT must(audit() = '', 'written while audit_enabled is off: ' || audit());

-- 2. On: CREATE with its definition, then one REFRESH per statement.
SET pg_tviews.audit_enabled = on;
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), label text);
SELECT pg_tviews_create('tv_tag', $$
    SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM tb_tag $$);
SELECT must(audit() = 'CREATE:tag:-', 'after create: ' || audit());
SELECT must((SELECT details->>'definition' LIKE '%tb_tag%' FROM tviews.pg_tview_audit_log
             WHERE operation = 'CREATE'), 'CREATE entry without its definition');
TRUNCATE tviews.pg_tview_audit_log;

UPDATE tb_user SET name = name || '!' WHERE pk_user <= 2;
SELECT must(audit() = 'REFRESH:user:2', 'after a 2-row UPDATE: ' || audit());
TRUNCATE tviews.pg_tview_audit_log;

-- 3. A rolled-back transaction leaves nothing, and buffers nothing for the next.
BEGIN;
UPDATE tb_user SET name = 'x';
ROLLBACK;
SELECT must(audit() = '', 'a rolled-back transaction was audited: ' || audit());

-- 4. A rolled-back savepoint's refresh is not audited; the rest of the
--    transaction is.
BEGIN;
UPDATE tb_user SET name = 'y' WHERE pk_user = 3;
SAVEPOINT s;
UPDATE tb_user SET name = 'z' WHERE pk_user IN (1, 2);
ROLLBACK TO SAVEPOINT s;
COMMIT;
SELECT must(audit() = 'REFRESH:user:1', 'after a rolled-back savepoint: ' || audit());
TRUNCATE tviews.pg_tview_audit_log;

-- 5. Entries buffered while off are discarded, not written by the next audited
--    statement.
SET pg_tviews.audit_enabled = off;
UPDATE tb_user SET name = 'w';
SET pg_tviews.audit_enabled = on;
INSERT INTO tb_tag (pk_tag, label) VALUES (1, 't');
SELECT must(audit() = 'REFRESH:tag:1', 'entries from an unaudited statement leaked: ' || audit());
TRUNCATE tviews.pg_tview_audit_log;

-- 6. DROP TABLE tv_x: one DROP.
DROP TABLE tv_tag;
SELECT must(audit() = 'DROP:tag:-', 'after DROP TABLE: ' || audit());

\echo 'audit_log: PASS'
