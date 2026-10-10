-- COPY FROM refreshes TVIEWs like INSERT does.
--
-- COPY writes without an ExecutorRun call for the copied rows, so the executor
-- frame stack sees no writing frame. Its row triggers still queue and its
-- statement-level flush trigger still flushes: the TVIEW, and a TVIEW embedding
-- it, are fresh after the COPY, inside and outside a transaction block.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_copy_from.sql
-- expect-output: copy from: all fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_author (pk_author int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author int REFERENCES tb_author, title text);
INSERT INTO tb_author VALUES (1, DEFAULT, 'ann');
SELECT pg_tviews_create('tv_author',
  $q$SELECT pk_author, id, jsonb_build_object('id', id, 'name', name) AS data FROM tb_author$q$);
SELECT pg_tviews_create('tv_post',
  $q$SELECT p.pk_post, p.id, p.fk_author,
            jsonb_build_object('id', p.id, 'title', p.title, 'author', a.data) AS data
     FROM tb_post p JOIN tv_author a ON a.pk_author = p.fk_author$q$);

COPY tb_author (pk_author, name) FROM STDIN;
2	bob
3	cid
\.
SELECT assert_fresh('tv_author', 'pk_author', 'COPY into tb_author');

COPY tb_post (pk_post, fk_author, title) FROM STDIN;
10	1	first
11	2	second
12	3	third
\.
SELECT assert_fresh('tv_post', 'pk_post', 'COPY into tb_post');

BEGIN;
COPY tb_author (pk_author, name) FROM STDIN;
4	dee
\.
UPDATE tb_author SET name = name || '!' WHERE pk_author IN (1, 4);
COPY tb_post (pk_post, fk_author, title) FROM STDIN;
13	4	fourth
\.
COMMIT;
SELECT assert_fresh('tv_author', 'pk_author', 'COPY and UPDATE in one transaction');
SELECT assert_fresh('tv_post', 'pk_post', 'COPY and UPDATE in one transaction');

\echo 'copy from: all fresh'
