-- The time refresh and the rebuild run in the TVIEW owner's frame, whoever calls
-- them: TVIEWs created by a role that is no superuser and owns no base table,
-- written by a third role from which row security hides some rows, refreshed
-- across a real time boundary by their owner, and rebuilt after a reset as the
-- rebuild worker does (a superuser calling pg_tviews_rebuild_all()). Freshness is
-- judged as the owner (assert_fresh).
--
-- The boundary is real time: a row is "recent" for 2 seconds after it is written.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_roles_time_and_rebuild.sql
-- expect-output: roles_time_and_rebuild: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'roles_time_and_rebuild FAIL: %', what; END IF; END $$;

DROP ROLE IF EXISTS regress_rt_owner;
DROP ROLE IF EXISTS regress_rt_writer;
CREATE ROLE regress_rt_owner BYPASSRLS;
CREATE ROLE regress_rt_writer;
GRANT CREATE ON SCHEMA public TO regress_rt_owner;

CREATE TABLE tb_shelf (pk_shelf bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_book (pk_book bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_shelf bigint NOT NULL REFERENCES tb_shelf, title text,
                      seen_at timestamptz NOT NULL DEFAULT now());
GRANT SELECT, TRIGGER ON tb_shelf, tb_book TO regress_rt_owner;
GRANT SELECT, INSERT, UPDATE, DELETE ON tb_shelf, tb_book TO regress_rt_writer;
-- The writer sees no odd shelf; the owner bypasses row security (pg_tviews refuses
-- a table whose row security applies to the owner).
ALTER TABLE tb_shelf ENABLE ROW LEVEL SECURITY;
CREATE POLICY writer_rows ON tb_shelf TO regress_rt_writer USING (pk_shelf % 2 = 0) WITH CHECK (true);
INSERT INTO tb_shelf (pk_shelf, name) VALUES (1, 'odd'), (2, 'even');
INSERT INTO tb_book (pk_book, fk_shelf, title, seen_at)
VALUES (1, 1, 'b1', now() - interval '1 hour'), (2, 2, 'b2', now() - interval '1 hour');

SET ROLE regress_rt_owner;
SELECT tviews.pg_tviews_create('tv_shelf', $$
    SELECT pk_shelf, id, jsonb_build_object('name', name) AS data FROM tb_shelf $$, '{"logged": false}');
SELECT tviews.pg_tviews_create('tv_book', $$
    SELECT b.pk_book, b.id, b.fk_shelf,
           jsonb_build_object('title', b.title, 'recent', b.seen_at > now() - interval '2 seconds',
                              'shelf', s.data) AS data
    FROM tb_book b JOIN tv_shelf s ON s.pk_shelf = b.fk_shelf $$,
    '{"logged": false, "time_refresh": "external"}');
RESET ROLE;
SELECT must(pg_get_userbyid(relowner) = 'regress_rt_owner', 'tv_book not owned by the owner role')
FROM pg_class WHERE oid = 'tv_book'::regclass;

-- 1. The writer's writes, to a row it sees and through a parent it does not.
SET ROLE regress_rt_writer;
UPDATE tb_book SET title = 'b1+', seen_at = now() WHERE pk_book = 1;
UPDATE tb_book SET title = 'b2+' WHERE pk_book = 2;
RESET ROLE;
-- The odd shelf is renamed by the owner of the table (the writer cannot see it).
UPDATE tb_shelf SET name = 'odd+' WHERE pk_shelf = 1;
SELECT assert_fresh('tv_shelf', 'pk_shelf', 'the writes');
SELECT assert_fresh('tv_book', 'pk_book', 'the writes');
SELECT must((SELECT (data->>'recent')::boolean FROM tv_book WHERE pk_book = 1), 'b1 not recent');

-- 2. The boundary passes: the view changes with no write, the TVIEW does not until
-- the time refresh runs. Without an argument it refreshes the TVIEWs the caller
-- owns: none for the writer; the owner's call (its pg_cron job) refreshes it.
SELECT pg_sleep(2.5);
SELECT must(fresh_diff('tv_book', 'pk_book') IS NOT NULL, 'the boundary passed unseen');
SET ROLE regress_rt_writer;
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_refresh_time_dependent()),
            'the writer refreshed a TVIEW it does not own');
SET ROLE regress_rt_owner;
SELECT must((SELECT array_agg(t) FROM tviews.pg_tviews_refresh_time_dependent() t) = '{public.tv_book}',
            'the owner''s time refresh');
RESET ROLE;
SELECT assert_fresh('tv_book', 'pk_book', 'the time refresh called by the owner');
SELECT must(NOT (SELECT (data->>'recent')::boolean FROM tv_book WHERE pk_book = 1), 'b1 still recent');

-- 3. A reset (as a crash empties every UNLOGGED table), rebuilt as the rebuild
-- worker does: a superuser calling pg_tviews_rebuild_all().
TRUNCATE tv_book, tv_shelf;
DELETE FROM tviews.pg_tview_valid;
SELECT must((SELECT array_agg(entity ORDER BY entity) FROM tviews.pg_tviews_rebuild_all()) = '{book,shelf}',
            'the reset TVIEWs were not rebuilt');
SELECT must((SELECT count(*) FROM tv_book) = 2, 'tv_book not refilled');
SELECT assert_fresh('tv_shelf', 'pk_shelf', 'the rebuild');
SELECT assert_fresh('tv_book', 'pk_book', 'the rebuild');

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_book, tb_shelf;
REVOKE CREATE ON SCHEMA public FROM regress_rt_owner;
DROP OWNED BY regress_rt_owner, regress_rt_writer;
DROP ROLE regress_rt_owner;
DROP ROLE regress_rt_writer;

SELECT 'roles_time_and_rebuild: PASS' AS result;
