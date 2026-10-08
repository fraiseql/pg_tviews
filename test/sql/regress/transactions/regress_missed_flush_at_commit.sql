-- Nothing queued survives a commit. Refresh work a transaction queued without
-- flushing fails the COMMIT with SQLSTATE 55000, every time: committing it would
-- leave the TVIEWs stale, and it is never applied later, in another
-- transaction, with a patch recorded earlier.
--
-- Runs in autocommit on purpose: each statement is its own transaction.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_missed_flush_at_commit.sql
--
-- expect-output: missed flush at commit: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_author (pk_author bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                        id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author bigint NOT NULL REFERENCES tb_author, title text);
INSERT INTO tb_author (name) VALUES ('ann'), ('ben');
INSERT INTO tb_post (fk_author, title) VALUES (1, 'p1'), (2, 'p2');

SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('title', p.title, 'author', a.name) AS data
    FROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author $$);

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post)
               WHERE t.pk_post IS NULL OR v.pk_post IS NULL
                  OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION '%: tv_post diverges from tviews.public__tv_post', label;
    END IF;
END $$;

-- ── a session starts with an empty queue ────────────────────────────────────
DO $$ BEGIN
    IF jsonb_array_length(tviews.pg_tviews_debug_queue()) <> 0 THEN
        RAISE EXCEPTION 'FAIL: work queued by an earlier transaction is still queued: %',
            tviews.pg_tviews_debug_queue();
    END IF;
END $$;

-- ── a commit with queued work fails, every time ─────────────────────────────
-- The row trigger records the change; tb_post's flush trigger is disabled, so
-- the statement reaches its commit with the work still queued.
DO $$
DECLARE flush_trigger name := (SELECT tgname FROM pg_trigger
                               WHERE tgrelid = 'tb_post'::regclass AND tgname LIKE 'trg_tview_flush_%');
BEGIN
    EXECUTE format('ALTER TABLE tb_post DISABLE TRIGGER %I', flush_trigger);
END $$;
\set ON_ERROR_STOP off
UPDATE tb_post SET title = 'stale' WHERE pk_post = 1;
\set ON_ERROR_STOP on
SELECT :'LAST_ERROR_SQLSTATE' = '55000' AS failed_loud \gset
\if :failed_loud
\else
    \echo 'FAIL: a commit with queued refreshes did not fail with 55000:' :'LAST_ERROR_SQLSTATE'
    \quit
\endif
\set ON_ERROR_STOP off
UPDATE tb_post SET title = 'stale again' WHERE pk_post = 2;
\set ON_ERROR_STOP on
SELECT :'LAST_ERROR_SQLSTATE' = '55000' AS failed_loud \gset
\if :failed_loud
\else
    \echo 'FAIL: the second commit with queued refreshes in a session did not fail:' :'LAST_ERROR_SQLSTATE'
    \quit
\endif
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tb_post WHERE title LIKE 'stale%') THEN
        RAISE EXCEPTION 'FAIL: a write whose refresh was never applied committed';
    END IF;
END $$;

-- ── the next transaction starts with an empty queue ─────────────────────────
DO $$ BEGIN
    IF jsonb_array_length(tviews.pg_tviews_debug_queue()) <> 0 THEN
        RAISE EXCEPTION 'FAIL: work queued by a failed transaction is still queued: %',
            tviews.pg_tviews_debug_queue();
    END IF;
END $$;
UPDATE tb_author SET name = 'ben (renamed)' WHERE pk_author = 2;
SELECT check_fresh('a write after the failed commits');


\echo 'missed flush at commit: PASS'
