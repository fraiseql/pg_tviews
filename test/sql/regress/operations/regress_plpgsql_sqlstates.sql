-- The extension's PL/pgSQL keeps the SQLSTATE of an error it cannot handle
-- (architecture review A3).
--
-- Two handlers caught every error: the cleanup after a dependent drop turned it
-- into a WARNING and the DROP committed with the TVIEW half deregistered;
-- pg_tviews_reregister_all(strict) raised P0001. A lock timeout must surface as
-- 55P03 from each, so a caller can retry. (A third, the check of a definition
-- rewritten after a column rename, went with the rewrite.)
--
-- A second session (dblink) holds an ACCESS EXCLUSIVE lock while this one runs with
-- a short lock_timeout.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_plpgsql_sqlstates.sql
-- expect-output: plpgsql_sqlstates: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE EXTENSION IF NOT EXISTS dblink;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'A3 FAIL: %', msg; END IF; END $$;

-- No foreign key: dropping tb_user then needs no lock on tb_post.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL, title text);
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 'p1');
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$) \g /dev/null

SELECT format('host=localhost dbname=%s user=%s port=%s', current_database(), current_user,
              current_setting('port')) AS conninfo \gset
SELECT dblink_connect('other', :'conninfo') \g /dev/null

-- 1. Dropping tb_user drops tv_post's backing view; the cleanup drops tv_post's
--    triggers on tb_post, which the other session has locked.
SELECT dblink_exec('other', 'BEGIN') \g /dev/null
SELECT dblink_exec('other', 'LOCK TABLE tb_post IN ACCESS EXCLUSIVE MODE') \g /dev/null
SET lock_timeout = '300ms';
\set ON_ERROR_STOP off
DROP TABLE tb_user CASCADE;
\set ON_ERROR_STOP on
SELECT :'LAST_ERROR_SQLSTATE' AS drop_state \gset
RESET lock_timeout;
SELECT dblink_exec('other', 'ROLLBACK') \g /dev/null
SELECT must(:'drop_state' = '55P03', 'the cleanup after a dependent drop gave '
            || :'drop_state' || ', not 55P03');
SELECT must(to_regclass('tb_user') IS NOT NULL
            AND EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'post'),
            'the DROP committed although its cleanup failed');

-- 2. pg_tviews_reregister_all(strict): re-registering installs triggers on a locked table.
SELECT dblink_exec('other', 'BEGIN') \g /dev/null
SELECT dblink_exec('other', 'LOCK TABLE tb_post IN ACCESS EXCLUSIVE MODE') \g /dev/null
SET lock_timeout = '300ms';
\set ON_ERROR_STOP off
SELECT * FROM tviews.pg_tviews_reregister_all(strict => true);
\set ON_ERROR_STOP on
SELECT :'LAST_ERROR_SQLSTATE' AS rereg_state \gset
RESET lock_timeout;
SELECT dblink_exec('other', 'ROLLBACK') \g /dev/null
SELECT must(:'rereg_state' = '55P03', 'pg_tviews_reregister_all(strict) gave '
            || :'rereg_state' || ', not 55P03');

SELECT dblink_disconnect('other') \g /dev/null
\echo 'plpgsql_sqlstates: PASS'
