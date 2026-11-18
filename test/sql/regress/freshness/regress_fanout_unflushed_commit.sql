-- A write whose only refresh work is a fan-out patch (no key queued) still fails
-- its COMMIT with 55000 when that work was never flushed: committing would leave
-- every document that copies the written value stale.
--
-- Runs in autocommit on purpose: the UPDATE is its own transaction.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_fanout_unflushed_commit.sql
-- allow-output: queued refreshes for
-- expect-output: unflushed fan-out at commit: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_author (pk_author int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author int NOT NULL REFERENCES tb_author, title text);
INSERT INTO tb_author (pk_author, name) VALUES (1, 'ann');
INSERT INTO tb_post (pk_post, fk_author, title) VALUES (1, 1, 'p1'), (2, 1, 'p2');
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', a.name)) AS data
    FROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author $$) IS NOT NULL AS created;

-- The author's name reaches tv_post by fan-out only: no key of tv_post is queued.
-- tb_author's flush trigger is disabled, so the UPDATE reaches its commit with
-- the patch pending.
DO $$
DECLARE flush_trigger name := (SELECT tgname FROM pg_trigger
                               WHERE tgrelid = 'tb_author'::regclass AND tgname LIKE 'trg_tview_flush_%');
BEGIN
    IF flush_trigger IS NULL THEN
        RAISE EXCEPTION 'FAIL: tb_author has no flush trigger';
    END IF;
    EXECUTE format('ALTER TABLE tb_author DISABLE TRIGGER %I', flush_trigger);
END $$;
\set ON_ERROR_STOP off
UPDATE tb_author SET name = 'bea' WHERE pk_author = 1;
\set ON_ERROR_STOP on
SELECT :'LAST_ERROR_SQLSTATE' AS sqlstate \gset
SELECT set_config('regress.sqlstate', :'sqlstate', false) IS NOT NULL AS noted;
DO $$ BEGIN
    IF current_setting('regress.sqlstate') <> '55000' THEN
        RAISE EXCEPTION 'FAIL: a commit with an unflushed fan-out patch ended with SQLSTATE % (want 55000); tb_author.name = %',
            current_setting('regress.sqlstate'), (SELECT name FROM tb_author WHERE pk_author = 1);
    END IF;
END $$;
SELECT assert_fresh('tv_post', 'pk_post', 'the refused commit');

SELECT 'unflushed fan-out at commit: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
