-- A user trigger on a TVIEW's table that writes a base table during the flush:
-- the flush refreshes what that write changed, and again every TVIEW reading
-- those rows, though it refreshed them earlier in the same flush. The parent used
-- to keep the child's row as it was before the write (tv_post's author "u2"
-- while tv_user and tb_user said "u2+").
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_trigger_writes_back_during_flush.sql
--
-- expect-output: trigger writes back during flush: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'u');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p'), (2, 1, 'q');
SELECT tviews.pg_tviews_create('tv_user', 'SELECT pk_user, id, jsonb_build_object(''name'', name) AS data FROM tb_user');
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);

-- Marks the author once: the second refresh of tv_post finds the mark and stops.
CREATE FUNCTION mark_author() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE public.tb_user SET name = name || '+'
     WHERE pk_user = NEW.fk_user AND name NOT LIKE '%+';
    RETURN NULL;
END $$;
CREATE TRIGGER mark_author AFTER UPDATE ON tv_post FOR EACH ROW EXECUTE FUNCTION mark_author();

UPDATE tb_user SET name = 'u2' WHERE pk_user = 1;
SELECT assert_fresh('tv_user', 'pk_user', 'after a write-back during the flush');
SELECT assert_fresh('tv_post', 'pk_post', 'after a write-back during the flush');

-- In a transaction, and from a write to the child's own table.
BEGIN;
UPDATE tb_post SET title = 'p2' WHERE pk_post = 1;
UPDATE tb_user SET name = 'u3' WHERE pk_user = 1;
COMMIT;
SELECT assert_fresh('tv_user', 'pk_user', 'after a transaction');
SELECT assert_fresh('tv_post', 'pk_post', 'after a transaction');

SELECT 'trigger writes back during flush: PASS' AS result;
