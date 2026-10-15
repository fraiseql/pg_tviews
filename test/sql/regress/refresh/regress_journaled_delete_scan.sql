-- Regression test (#237): a refresh of several keys reads the backing view once.
--
-- A statement that changes the rows of several TVIEW keys refreshes them in one
-- batch: an upsert from the backing view, then a delete of the keys the view no
-- longer produces. The delete asked the view again, through
-- `NOT EXISTS (SELECT 1 FROM view v WHERE v.key = t.key)`, and the planner can't
-- push that correlation below a GROUP BY: an aggregate TVIEW computed every one
-- of its groups, reading every child row, on each multi-key write.
--
-- The children table is large enough (5,000 posts × 2 tags) that every other
-- statement of the refresh looks the keys up through an index on cost, so any
-- sequential scan of tb_tag during the write belongs to the delete. Scans are
-- counted in pg_stat_xact_user_tables inside the write's own transaction.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_journaled_delete_scan.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\ir ../../lib/assert_fresh.sql

-- The sequential scans of `rel` while `write` runs, in the caller's transaction.
CREATE FUNCTION seq_scans_during(rel regclass, write text) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE before bigint;
BEGIN
    SELECT seq_scan INTO before FROM pg_stat_xact_user_tables WHERE relid = rel;
    EXECUTE write;
    RETURN (SELECT seq_scan FROM pg_stat_xact_user_tables WHERE relid = rel) - before;
END $$;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#237 FAIL: %', what; END IF; END $$;

-- The write in its own transaction: no sequential scan of tb_tag, tv_post fresh.
CREATE FUNCTION check_write(label text, write text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE scans bigint := seq_scans_during('tb_tag', write);
BEGIN
    PERFORM must(scans = 0, format('%s: the refresh scanned tb_tag %s time(s)', label, scans));
    PERFORM assert_fresh('tv_post', 'pk_post', label);
END $$;

CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text NOT NULL);
CREATE TABLE tb_label (pk_label int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       label text NOT NULL);
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     fk_post int NOT NULL REFERENCES tb_post,
                     fk_label int NOT NULL REFERENCES tb_label, name text NOT NULL);
CREATE INDEX ON tb_tag (fk_post);
INSERT INTO tb_post (pk_post, title) SELECT g, 'post ' || g FROM generate_series(1, 5000) g;
INSERT INTO tb_label (pk_label, label) SELECT g, 'label ' || g FROM generate_series(1, 10) g;
INSERT INTO tb_tag (pk_tag, fk_post, fk_label, name)
    SELECT (p - 1) * 2 + i, p, 1 + i % 10, 'tag'
    FROM generate_series(1, 5000) p, generate_series(1, 2) i;

-- An aggregate TVIEW: each post with its tags, grouped.
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id,
           jsonb_build_object('title', p.title,
                              'tags', jsonb_agg(jsonb_build_object('name', t.name, 'label', l.label)
                                                ORDER BY t.pk_tag)) AS data
    FROM tb_post p
    LEFT JOIN tb_tag t ON t.fk_post = p.pk_post
    LEFT JOIN tb_label l ON l.pk_label = t.fk_label
    GROUP BY p.pk_post, p.id, p.title $$);
ANALYZE tb_post, tb_label, tb_tag, tv_post;

-- 1. One post: the single-key refresh never asked the view twice.
SELECT check_write('one post, READ COMMITTED',
                   $$UPDATE tb_tag SET name = 'one' WHERE pk_tag = 5$$);

-- 2. Two and five posts in one statement: the batch refresh.
SELECT check_write('two posts, READ COMMITTED',
                   $$UPDATE tb_tag SET name = 'two' WHERE pk_tag IN (5, 7)$$);
SELECT check_write('five posts, READ COMMITTED',
                   $$UPDATE tb_tag SET name = 'five' WHERE pk_tag IN (1, 5, 7, 9, 11)$$);

-- 3. The same at REPEATABLE READ.
BEGIN ISOLATION LEVEL REPEATABLE READ;
SELECT check_write('two posts, REPEATABLE READ',
                   $$UPDATE tb_tag SET name = 'rr' WHERE pk_tag IN (5, 7)$$);
COMMIT;

-- 4. Posts deleted by one statement: both TVIEW rows go, the others stay.
DELETE FROM tb_tag WHERE fk_post IN (3, 4);
SELECT check_write('two posts deleted',
                   $$DELETE FROM tb_post WHERE pk_post IN (3, 4)$$);
SELECT must(NOT EXISTS (SELECT 1 FROM tv_post WHERE pk_post IN (3, 4)),
            'the deleted posts left rows in tv_post');
SELECT must((SELECT count(*) FROM tv_post) = 4998, 'tv_post lost rows it should keep');

-- 5. A batch where some keys stay and some go, in one statement: a writable CTE
--    deletes post 6's tags and post 6, and renames post 8.
SELECT check_write('one post kept, one deleted', $$
    WITH gone AS (DELETE FROM tb_tag WHERE fk_post = 6 RETURNING fk_post),
         renamed AS (UPDATE tb_post SET title = 'eight' WHERE pk_post = 8 RETURNING pk_post)
    DELETE FROM tb_post WHERE pk_post IN (SELECT fk_post FROM gone)$$);
SELECT must(NOT EXISTS (SELECT 1 FROM tv_post WHERE pk_post = 6), 'post 6 left a row');
SELECT must((SELECT data->>'title' FROM tv_post WHERE pk_post = 8) = 'eight',
            'post 8 was not refreshed');

-- 6. Other shapes through the batch refresh: a UNION, a DISTINCT ON over a uuid
--    (a text identity) and a plain TVIEW stay equal to their views after
--    multi-key writes and deletes.
CREATE TABLE tb_task (pk_task int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text NOT NULL);
CREATE TABLE tb_task_archive (pk_task int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                              title text NOT NULL);
INSERT INTO tb_task (pk_task, title) SELECT g, 'task ' || g FROM generate_series(1, 10) g;
INSERT INTO tb_task_archive (pk_task, title) SELECT g, 'old ' || g FROM generate_series(11, 20) g;
SELECT tviews.pg_tviews_create('tv_task', $$
    SELECT pk_task, id, jsonb_build_object('title', title) AS data FROM tb_task
    UNION ALL
    SELECT pk_task, id, jsonb_build_object('title', title) AS data FROM tb_task_archive $$);

CREATE TABLE tb_price (pk_price int PRIMARY KEY, id uuid NOT NULL, amount int NOT NULL,
                       valid_from int NOT NULL);
INSERT INTO tb_price
    SELECT g, ('00000000-0000-0000-0000-' || lpad((1 + g % 5)::text, 12, '0'))::uuid, g, g
    FROM generate_series(1, 20) g;
SELECT tviews.pg_tviews_create('tv_price', $$
    SELECT DISTINCT ON (p.id) p.pk_price, p.id, jsonb_build_object('amount', p.amount) AS data
    FROM tb_price p ORDER BY p.id, p.valid_from DESC $$);

CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      body text NOT NULL);
INSERT INTO tb_note (pk_note, body) SELECT g, 'note ' || g FROM generate_series(1, 10) g;
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT n.pk_note, n.id, jsonb_build_object('body', n.body) AS data FROM tb_note n $$);

UPDATE tb_task SET title = 'renamed' WHERE pk_task IN (1, 2, 3);
DELETE FROM tb_task WHERE pk_task IN (4, 5);
UPDATE tb_task_archive SET title = 'gone' WHERE pk_task IN (11, 12);
DELETE FROM tb_task_archive WHERE pk_task IN (13, 14);
SELECT assert_fresh('tv_task', 'pk_task', 'UNION: multi-key writes and deletes');

UPDATE tb_price SET amount = -amount WHERE pk_price IN (16, 17, 18);
DELETE FROM tb_price WHERE pk_price IN (19, 20);
SELECT assert_fresh('tv_price', 'id', 'DISTINCT ON uuid: newest prices deleted');
DELETE FROM tb_price WHERE id = '00000000-0000-0000-0000-000000000002'
                        OR id = '00000000-0000-0000-0000-000000000003';
SELECT assert_fresh('tv_price', 'id', 'DISTINCT ON uuid: two keys deleted whole');

UPDATE tb_note SET body = 'x' WHERE pk_note IN (1, 2);
DELETE FROM tb_note WHERE pk_note IN (3, 4, 5);
SELECT assert_fresh('tv_note', 'pk_note', 'plain: multi-key writes and deletes');

\echo 'journaled delete scan (#237): PASS'
