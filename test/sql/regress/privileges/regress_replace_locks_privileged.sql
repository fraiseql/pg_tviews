-- Replacing a TVIEW in place locks, against writers, only the tables its caller
-- may attach triggers to (reading a table to recompute a dependent holds no
-- writer off): the base table of another role's TVIEW reading it is
-- left to that TVIEW's own triggers and table lock, and the dependent is still
-- recomputed.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_replace_locks_privileged.sql
-- expect-output: replace_locks_privileged: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP TABLE IF EXISTS tb_item, tb_note, tv_item, tv_note CASCADE;
DROP ROLE IF EXISTS regress_rlp_owner;
DROP ROLE IF EXISTS regress_rlp_reader;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'FAIL: %', msg; END IF; END $$;

CREATE ROLE regress_rlp_owner;
CREATE ROLE regress_rlp_reader;
GRANT CREATE ON SCHEMA public TO regress_rlp_owner, regress_rlp_reader;

SET ROLE regress_rlp_owner;
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text);
INSERT INTO tb_item (pk_item, title) VALUES (1, 'a');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('title', title) AS data FROM tb_item $$) \g /dev/null
GRANT SELECT, TRIGGER ON tv_item TO regress_rlp_reader;

SET ROLE regress_rlp_reader;
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_item int, body text);
INSERT INTO tb_note (pk_note, fk_item, body) VALUES (1, 1, 'private');
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT n.pk_note, n.id, jsonb_build_object('body', n.body, 'item', i.data) AS data
    FROM tb_note n LEFT JOIN tv_item i ON i.pk_item = n.fk_item $$) \g /dev/null

SET ROLE regress_rlp_owner;
SELECT must(NOT has_table_privilege('tb_note', 'TRIGGER'), 'the owner may trigger tb_note');
BEGIN;
SELECT tviews.pg_tviews_create_or_replace('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('title', upper(title)) AS data FROM tb_item $$)
    \g /dev/null
SELECT must(NOT EXISTS (SELECT FROM pg_catalog.pg_locks
                        WHERE pid = pg_backend_pid() AND relation = 'tb_note'::regclass
                          AND mode <> 'AccessShareLock'),
            'replacing tv_item locked tb_note, which its caller may not attach triggers to');
SELECT must(EXISTS (SELECT FROM pg_catalog.pg_locks
                    WHERE pid = pg_backend_pid() AND relation = 'tb_item'::regclass
                      AND mode = 'ShareLock'),
            'replacing tv_item did not lock its own base table');
COMMIT;
RESET ROLE;
SELECT must((SELECT data->'item'->>'title' FROM tv_note WHERE pk_note = 1) = 'A',
            'tv_note was not recomputed after tv_item was replaced');

DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY regress_rlp_owner, regress_rlp_reader;
REVOKE CREATE ON SCHEMA public FROM regress_rlp_owner, regress_rlp_reader;
DROP ROLE regress_rlp_owner;
DROP ROLE regress_rlp_reader;
\echo 'replace_locks_privileged: PASS'
