-- Regression test (#231): publishing changed TVIEW rows at commit (ADR 0231).
--
-- The delivery itself (one message per committed transaction, nothing on
-- rollback, net changes) is pinned by test/isolation/specs/publish-changes.spec.
-- Here: the message cap of a large transaction, `types` mode, PREPARE TRANSACTION
-- refused while there are changes to publish, and who may set the setting.
-- Notifications are counted in a second psql that LISTENs in its own session:
-- psql prints each one it receives.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_publish_changes.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

SELECT current_setting('max_prepared_transactions')::int = 0 AS no_2pc \gset
\if :no_2pc
  \echo 'SKIP: max_prepared_transactions = 0'
  \quit
\endif

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#231 FAIL: %', what; END IF; END $$;

-- The SQLSTATE and message `stmt` fails with, NULL when it succeeds.
CREATE FUNCTION failure(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      n int NOT NULL);
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, label text NOT NULL);
INSERT INTO tb_tag VALUES (1, 'x');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('n', i.n) AS data FROM tb_item i $$);
SELECT tviews.pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, jsonb_build_object('label', t.label) AS data FROM tb_tag t $$);

\setenv PGTV_DB :DBNAME

-- 1. A 2,000-row transaction: 8 messages, each under the payload limit, the last
--    naming the type whose ids did not fit.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "INSERT INTO tb_item (pk_item, n) SELECT g, g FROM generate_series(1, 2000) g" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/'`
CREATE TEMP TABLE notes AS
SELECT n AS payload FROM regexp_split_to_table(:'notes', E'\n') n WHERE n <> '';
SELECT must((SELECT count(*) FROM notes) = 8, (SELECT count(*) FROM notes) || ' messages');
SELECT must(bool_and(octet_length(payload) < 8000 AND (payload::jsonb->>'v') = '2'),
            'a message over the limit or not v2')
FROM notes;
SELECT must((SELECT count(*) FROM notes, jsonb_array_elements_text(payload::jsonb->'u'->'Item')) > 1000
            AND (SELECT count(*) FROM notes WHERE payload::jsonb->'t' = '["Item"]') = 1,
            'ids then the type by name');

-- 2. `types` mode: the types only.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = types" -c "UPDATE tb_item SET n = n + 1 WHERE pk_item <= 3" -c "UPDATE tb_tag SET label = 'y'" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = '{"v":2,"t":["Item"]}|{"v":2,"t":["Tag"]}', 'types mode: ' || :'notes');

-- 2b. A bulk change made with triggers suspended is rebuilt at resume, rows not
--     listed: its type is published by name. So is a pg_tviews_refresh().
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "BEGIN" -c "SELECT tviews.pg_tviews_suspend_triggers()" -c "UPDATE tb_item SET n = n + 1" -c "SELECT tviews.pg_tviews_resume_triggers()" -c "COMMIT" -c "SELECT tviews.pg_tviews_refresh('tv_item')" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = '{"v":2,"t":["Item"]}|{"v":2,"t":["Item"]}', 'rebuilds: ' || :'notes');

-- 3. PREPARE TRANSACTION with changes to publish is refused; without changes, or
--    with publishing off, it prepares.
SET pg_tviews.publish_changes = rows;
BEGIN;
UPDATE tb_item SET n = 0 WHERE pk_item = 1;
\set ON_ERROR_STOP off
PREPARE TRANSACTION 'publish_refused';
\set ON_ERROR_STOP on
SELECT must(:'LAST_ERROR_SQLSTATE' = '25000' AND :'LAST_ERROR_MESSAGE' LIKE '%publish_changes%',
            'PREPARE with changes: ' || :'LAST_ERROR_SQLSTATE' || ' ' || :'LAST_ERROR_MESSAGE');
SELECT must(NOT EXISTS (SELECT 1 FROM pg_prepared_xacts WHERE gid = 'publish_refused'),
            'the transaction was prepared');
SELECT must((SELECT n FROM tb_item WHERE pk_item = 1) <> 0, 'the refused transaction committed');
BEGIN;
SELECT count(*) FROM tv_item;
PREPARE TRANSACTION 'nothing_to_publish';
COMMIT PREPARED 'nothing_to_publish';
SET pg_tviews.publish_changes = off;
BEGIN;
UPDATE tb_item SET n = 0 WHERE pk_item = 1;
PREPARE TRANSACTION 'publishing_off';
COMMIT PREPARED 'publishing_off';
SELECT must((SELECT data->>'n' FROM tv_item WHERE pk_item = 1) = '0', 'the prepared write');

-- 4. Only a superuser sets it.
CREATE ROLE publish_changes_user;
SET ROLE publish_changes_user;
SELECT failure($$SET pg_tviews.publish_changes = rows$$) AS err \gset
RESET ROLE;
SELECT must(:'err' LIKE '42501 %', 'a non-superuser set it: ' || coalesce(:'err', 'it succeeded'));
DROP ROLE publish_changes_user;
RESET pg_tviews.publish_changes;
SELECT must(current_setting('pg_tviews.publish_changes') = 'off', 'the default is not off');

\echo 'publish_changes (#231): PASS'
