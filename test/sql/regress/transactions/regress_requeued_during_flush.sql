-- A key queued again by a trigger the flush's own writes fire is refreshed again,
-- and so is every TVIEW reading it.
--
-- tv_post embeds tv_user, and a user trigger on tv_post's table writes back to
-- tb_user. A rename refreshes tv_user, then tv_post; tv_post's trigger then
-- changes the user row tv_user already refreshed. The flush once dropped that new
-- work (tv_user stale), then refreshed tv_user but not tv_post (tv_post stale).
-- Here the trigger counts every refresh of a post into the author row the post
-- embeds, so the TVIEWs can never agree with their views: the flush refreshes
-- until its step limit and the write fails, changing nothing. A write-back that
-- settles is regress_trigger_writes_back_during_flush.sql.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_requeued_during_flush.sql
-- expect-output: requeued_during_flush: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, posts_touched int NOT NULL DEFAULT 0);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'touched', posts_touched) AS data
    FROM tb_user
$$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user
$$);

-- Counts every refresh of a post into its author's row. Runs inside the flush,
-- whose search_path is pg_catalog, pg_temp: names are qualified.
CREATE FUNCTION public.touch_author() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE public.tb_user SET posts_touched = posts_touched + 1 WHERE pk_user = NEW.fk_user;
    RETURN NULL;
END $$;
CREATE TRIGGER touch_author AFTER UPDATE ON tv_post
    FOR EACH ROW WHEN (OLD.data IS DISTINCT FROM NEW.data) EXECUTE FUNCTION public.touch_author();

CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    -- A write run inside a function is refreshed when the outer statement ends
    -- (#286): flush it here.
    PERFORM tviews.pg_tviews_flush_and_report(include_data => false);
    RETURN 'committed';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;
SELECT error_of($$UPDATE tb_user SET name = 'grace' WHERE pk_user = 1$$) AS outcome \gset
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'requeued FAIL: %', what; END IF; END $$;
SELECT must(:'outcome' LIKE '54001 %propagation depth%',
            'a write that never settles was not refused: ' || :'outcome');
SELECT assert_fresh('tv_user', 'pk_user', 'after the refused rename');
SELECT assert_fresh('tv_post', 'pk_post', 'after the refused rename');

\echo 'requeued_during_flush: PASS'
