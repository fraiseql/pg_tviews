-- A refresh write that fires another flush does not run a flush inside a flush.
--
-- A TVIEW's table read by another TVIEW carries pg_tviews' triggers like a base
-- table. When the flush at COMMIT (or after an admin rebuild) refreshes such a
-- table, the table's statement trigger fires inside the running flush. It must
-- leave the work to the flush already running: a flush inside it would drain the
-- queue and reset what the outer flush knows about the rows it changed, which
-- decides where propagation goes next.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_nested_flush.sql
-- expect-output: nested flush: all fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- user <- post <- feed: post embeds its author, feed reads tv_post's table.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int, title text);
INSERT INTO tb_user SELECT g, gen_random_uuid(), 'u' || g FROM generate_series(1, 3) g;
INSERT INTO tb_post SELECT g, gen_random_uuid(), 1 + g % 3, 't' || g FROM generate_series(1, 9) g;
SELECT pg_tviews_create('tv_user',
  $q$SELECT pk_user, id, jsonb_build_object('id', id, 'name', name) AS data FROM tb_user$q$);
SELECT pg_tviews_create('tv_post',
  $q$SELECT p.pk_post, p.id, p.fk_user,
            jsonb_build_object('id', p.id, 'title', p.title, 'author', u.data) AS data
     FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user$q$);
SELECT pg_tviews_create('tv_feed',
  $q$SELECT u.pk_user AS pk_feed, u.id,
            jsonb_build_object('posts', (SELECT jsonb_agg(p.data ORDER BY p.pk_post)
                                         FROM tv_post p WHERE p.fk_user = u.pk_user)) AS data
     FROM tb_user u$q$);

-- Writes while suspended: COMMIT catches up, rebuilding and then flushing outside
-- any statement.
BEGIN;
SELECT pg_tviews_suspend_triggers();
UPDATE tb_user SET name = 'renamed' WHERE pk_user = 2;
UPDATE tb_post SET title = 'retitled' WHERE pk_post = 4;
COMMIT;
SELECT assert_fresh('tv_user', 'pk_user', 'the COMMIT flush');
SELECT assert_fresh('tv_post', 'pk_post', 'the COMMIT flush');
SELECT assert_fresh('tv_feed', 'pk_feed', 'the COMMIT flush');

-- An admin rebuild flushes what it queued.
UPDATE tb_user SET name = 'again' WHERE pk_user = 3;
SELECT pg_tviews_refresh('user');
SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_refresh');
SELECT assert_fresh('tv_feed', 'pk_feed', 'pg_tviews_refresh');

-- A user trigger on a TVIEW's table that writes a base table: under a flush no
-- writing statement encloses (a TRUNCATE runs no executor), the refresh write
-- (run as the TVIEW owner, with search_path pg_catalog, pg_temp)
-- to tv_account fires it, and the base table's flush trigger fires inside the
-- running flush. It leaves the work to that flush: one flush in all.
CREATE TABLE tb_account (pk_account int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_log (pk_log serial PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), what text);
INSERT INTO tb_account SELECT g, gen_random_uuid(), 'a' || g FROM generate_series(1, 3) g;
SELECT pg_tviews_create('tv_account',
  $q$SELECT pk_account, id, jsonb_build_object('name', name) AS data FROM tb_account$q$);
SELECT pg_tviews_create('tv_log',
  $q$SELECT pk_log, id, jsonb_build_object('what', what) AS data FROM tb_log$q$);
CREATE FUNCTION log_account_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO public.tb_log (what) VALUES (TG_OP || ' tv_account');
    RETURN NULL;
END $$;
CREATE TRIGGER log_account_change AFTER INSERT OR UPDATE OR DELETE ON tv_account
    FOR EACH STATEMENT EXECUTE FUNCTION log_account_change();
DO $$
DECLARE before bigint := (pg_tviews_queue_stats()->>'flushes')::bigint;
BEGIN
    TRUNCATE tb_account;
    IF (pg_tviews_queue_stats()->>'flushes')::bigint - before IS DISTINCT FROM 1 THEN
        RAISE EXCEPTION 'expected one flush for the TRUNCATE, got %',
            (pg_tviews_queue_stats()->>'flushes')::bigint - before;
    END IF;
END $$;
SELECT assert_fresh('tv_account', 'pk_account', 'TRUNCATE tb_account');
SELECT assert_fresh('tv_log', 'pk_log', 'TRUNCATE tb_account');

\echo 'nested flush: all fresh'
