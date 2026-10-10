-- Nothing queued survives a commit. pg_tviews_cascade() run in autocommit (no
-- statement trigger follows a SELECT) refreshes before it returns. Refresh work a
-- transaction queued without flushing is dropped at COMMIT with a WARNING; it is
-- never applied later, in another transaction, with a patch recorded earlier.
--
-- Runs in autocommit on purpose: each statement is its own transaction.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_missed_flush_at_commit.sql
--
-- expect-output: missed flush at commit: PASS
-- expect-once: queued refreshes

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

-- ── N2: the next transaction starts with an empty queue ─────────────────────
DO $$ BEGIN
    IF jsonb_array_length(tviews.pg_tviews_debug_queue()) <> 0 THEN
        RAISE EXCEPTION 'N2 FAIL: work queued by an earlier transaction is still queued: %',
            tviews.pg_tviews_debug_queue();
    END IF;
END $$;

-- ── N2: a patch recorded in one transaction is never applied in another ─────
-- The row trigger records the change (with a direct patch); tb_post's flush
-- trigger is disabled, so the statement commits with the work still queued.
DO $$
DECLARE flush_trigger name := (SELECT tgname FROM pg_trigger
                               WHERE tgrelid = 'tb_post'::regclass AND tgname LIKE 'trg_tview_flush_%');
BEGIN
    EXECUTE format('ALTER TABLE tb_post DISABLE TRIGGER %I', flush_trigger);
END $$;
UPDATE tb_post SET title = 'stale' WHERE pk_post = 1;
-- The row changes again, unseen, then a write to tb_author flushes.
SET session_replication_role = replica;
UPDATE tb_post SET title = 'newer' WHERE pk_post = 1;
RESET session_replication_role;
UPDATE tb_author SET name = 'ben (renamed)' WHERE pk_author = 2;
DO $$ BEGIN
    IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) = 'stale' THEN
        RAISE EXCEPTION 'N2 FAIL: a patch recorded by an earlier transaction was applied later';
    END IF;
END $$;
SELECT pg_tviews_refresh('post');
SELECT check_fresh('control: after a full refresh');

\echo 'missed flush at commit: PASS'
