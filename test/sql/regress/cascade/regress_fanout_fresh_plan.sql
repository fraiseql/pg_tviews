-- Regression test (#235): a write looks its children up through the lookup
-- index on a freshly built TVIEW.
--
-- The statements that find a parent's children (the fan-out UPDATE, its
-- REPEATABLE READ cross-check, parent discovery) took the keys as an array and
-- ran from a plan made once, without them. That plan assumed ten keys, each
-- matching the average number of rows per lookup value; when the lookup column
-- has few distinct values (categories, statuses, small parents) it estimated a
-- large share of the table and hash-joined over a sequential scan of the whole
-- child TVIEW, on every write. One key is now looked up by `= $1` from a plan
-- whose estimate is one key's, and several keys are planned for their count.
--
-- Each write is counted in pg_stat_xact_user_tables inside its own transaction:
-- the backend's pending counters reach the shared statistics only once it is
-- idle outside a transaction, so a before/after delta there is exact, with no
-- timing and no wait.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_fanout_fresh_plan.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- `cats` categories of `per` items each, tv_item embedding the category's name
-- (copied from tb_cat, or tv_cat's document when `embedded`), freshly analyzed.
CREATE FUNCTION build(cats int, per int, embedded boolean) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF to_regclass('tv_item') IS NOT NULL THEN PERFORM tviews.pg_tviews_drop('tv_item'); END IF;
    IF to_regclass('tv_cat') IS NOT NULL THEN PERFORM tviews.pg_tviews_drop('tv_cat'); END IF;
    DROP TABLE IF EXISTS tb_item, tb_cat;
    CREATE TABLE tb_cat (pk_cat int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text NOT NULL);
    CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          fk_cat int NOT NULL REFERENCES tb_cat, title text NOT NULL);
    CREATE INDEX ON tb_item (fk_cat);
    INSERT INTO tb_cat (pk_cat, name) SELECT g, 'cat ' || g FROM generate_series(1, cats) g;
    INSERT INTO tb_item (pk_item, fk_cat, title)
        SELECT (c - 1) * per + i, c, repeat('t', 300)
        FROM generate_series(1, cats) c, generate_series(1, per) i;
    PERFORM tviews.pg_tviews_create('tv_cat', $q$
        SELECT c.pk_cat, c.id, jsonb_build_object('id', c.id, 'name', c.name) AS data
        FROM tb_cat c $q$);
    IF embedded THEN
        PERFORM tviews.pg_tviews_create('tv_item', $q$
            SELECT i.pk_item, i.id, i.fk_cat,
                   jsonb_build_object('title', i.title, 'category', c.data) AS data
            FROM tb_item i JOIN tv_cat c ON c.pk_cat = i.fk_cat $q$);
    ELSE
        PERFORM tviews.pg_tviews_create('tv_item', $q$
            SELECT i.pk_item, i.id, i.fk_cat,
                   jsonb_build_object('title', i.title,
                                      'category', jsonb_build_object('name', c.name)) AS data
            FROM tb_item i JOIN tb_cat c ON c.pk_cat = i.fk_cat $q$);
    END IF;
    ANALYZE tb_cat, tb_item, tv_cat, tv_item;
END $$;

-- The sequential scans of `rel` while `write` runs, in the caller's transaction.
CREATE FUNCTION seq_scans_during(rel regclass, write text) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE before bigint;
BEGIN
    SELECT seq_scan INTO before FROM pg_stat_xact_user_tables WHERE relid = rel;
    EXECUTE write;
    RETURN (SELECT seq_scan FROM pg_stat_xact_user_tables WHERE relid = rel) - before;
END $$;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#235 FAIL: %', what; END IF; END $$;

-- Every row of tv_item is the one its backing view computes.
CREATE FUNCTION fresh() RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    RETURN NOT EXISTS ((SELECT pk_item, data FROM tv_item
                        EXCEPT SELECT pk_item, data FROM tviews.public__tv_item)
                       UNION ALL (SELECT pk_item, data FROM tviews.public__tv_item
                                  EXCEPT SELECT pk_item, data FROM tv_item));
END $$;

-- The write in its own transaction: no sequential scan of tv_item, children fresh.
CREATE FUNCTION check_write(label text, write text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE scans bigint := seq_scans_during('tv_item', write);
BEGIN
    PERFORM must(scans = 0, format('%s: %s seq scan(s) of tv_item', label, scans));
    PERFORM must(fresh(), format('%s: tv_item is stale', label));
END $$;

-- 1. One category at READ COMMITTED: the fan-out UPDATE.
SELECT build(20, 50, false);
SELECT check_write('one key, READ COMMITTED',
                   $$UPDATE tb_cat SET name = 'one' WHERE pk_cat = 2$$);
SELECT must(count(*) = 50, 'one key: the 50 children hold the new name')
FROM tv_item WHERE data #>> '{category,name}' = 'one';

-- 2. Two categories in one statement: one fan-out UPDATE for both keys.
SELECT build(100, 20, false);
SELECT check_write('two keys, READ COMMITTED',
                   $$UPDATE tb_cat SET name = 'two' WHERE pk_cat IN (2, 3)$$);
SELECT must(count(*) = 40, 'two keys: the 40 children hold the new name')
FROM tv_item WHERE data #>> '{category,name}' = 'two';

-- 3. Both shapes write exactly the children a key has when the statement ends:
--    none, or children another write in the same statement moved to it. The
--    patches counted include the categories' own tv_cat rows.
CREATE FUNCTION patched_during(write text) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE before bigint := (tviews.pg_tviews_queue_stats()->>'direct_patches_applied')::bigint;
BEGIN
    EXECUTE write;
    RETURN (tviews.pg_tviews_queue_stats()->>'direct_patches_applied')::bigint - before;
END $$;
DELETE FROM tb_item WHERE fk_cat IN (5, 6);
SELECT must(patched_during($$UPDATE tb_cat SET name = 'none' WHERE pk_cat = 5$$) = 1,
            'one key without children: only its tv_cat row written');
SELECT must(patched_during($$UPDATE tb_cat SET name = 'some' WHERE pk_cat IN (6, 7)$$) = 22,
            'two keys, one without children: their tv_cat rows and the 20 children written');
SELECT must(fresh(), 'keys without children: tv_item is stale');
WITH moved AS (UPDATE tb_item SET fk_cat = 9 WHERE fk_cat = 8 RETURNING 1)
UPDATE tb_cat SET name = 'moved' WHERE pk_cat = 9;
SELECT must(count(*) = 40, 'one key: the children moved to it hold its new name')
FROM tv_item WHERE fk_cat = 9 AND data #>> '{category,name}' = 'moved';
WITH moved AS (UPDATE tb_item SET fk_cat = 11 WHERE fk_cat = 10 RETURNING 1)
UPDATE tb_cat SET name = 'moved too' WHERE pk_cat IN (11, 12);
SELECT must(count(*) = 60, 'two keys: the children moved to them hold their new name')
FROM tv_item WHERE fk_cat IN (11, 12) AND data #>> '{category,name}' = 'moved too';
SELECT must(fresh(), 'moved children: tv_item is stale');

-- 4. One category at REPEATABLE READ: the fan-out and its cross-check, which
--    looks the children up again under the transaction's and the latest snapshot.
SELECT build(20, 50, false);
BEGIN ISOLATION LEVEL REPEATABLE READ;
SELECT check_write('one key, REPEATABLE READ',
                   $$UPDATE tb_cat SET name = 'three' WHERE pk_cat = 2$$);
COMMIT;

-- 5. tv_item embedding tv_cat's document: parent discovery by fk_cat, cross-checked
--    at REPEATABLE READ, with and without direct patches.
SELECT build(20, 50, true);
BEGIN ISOLATION LEVEL REPEATABLE READ;
SELECT check_write('embedded TVIEW, REPEATABLE READ, patched',
                   $$UPDATE tb_cat SET name = 'four' WHERE pk_cat = 2$$);
COMMIT;
SET pg_tviews.direct_patch_enabled = off;
BEGIN ISOLATION LEVEL REPEATABLE READ;
SELECT check_write('embedded TVIEW, REPEATABLE READ, recomputed',
                   $$UPDATE tb_cat SET name = 'five' WHERE pk_cat = 3$$);
COMMIT;
RESET pg_tviews.direct_patch_enabled;

-- 6. tv_post aggregating its tags, each with its label: a refresh of a post
--    locks the labels its tags read (ADR 0207), found through its 50 tags.
SELECT tviews.pg_tviews_drop('tv_item'), tviews.pg_tviews_drop('tv_cat');
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text NOT NULL);
CREATE TABLE tb_label (pk_label int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       label text NOT NULL);
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     fk_post int NOT NULL REFERENCES tb_post,
                     fk_label int NOT NULL REFERENCES tb_label, name text NOT NULL);
CREATE INDEX ON tb_tag (fk_post);
INSERT INTO tb_post (pk_post, title) SELECT g, 'post ' || g FROM generate_series(1, 20) g;
INSERT INTO tb_label (pk_label, label) SELECT g, 'label ' || g FROM generate_series(1, 10) g;
INSERT INTO tb_tag (pk_tag, fk_post, fk_label, name)
    SELECT (p - 1) * 50 + i, p, 1 + i % 10, repeat('n', 300)
    FROM generate_series(1, 20) p, generate_series(1, 50) i;
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
SELECT must(seq_scans_during('tb_tag', $$UPDATE tb_tag SET name = 'six' WHERE pk_tag = 60$$) = 0,
            'one post, READ COMMITTED: its read set scanned tb_tag');

-- 7. At REPEATABLE READ the refreshed post is read again from its backing view,
--    under the transaction's snapshot and the latest one, and compared.
BEGIN ISOLATION LEVEL REPEATABLE READ;
SELECT must(seq_scans_during('tb_tag', $$UPDATE tb_tag SET name = 'seven' WHERE pk_tag = 160$$) = 0,
            'one post, REPEATABLE READ: its refreshed rows scanned tb_tag');
COMMIT;
SELECT must(NOT EXISTS (SELECT pk_post, data FROM tv_post
                        EXCEPT SELECT pk_post, data FROM tviews.public__tv_post),
            'tv_post is stale');
SELECT tviews.pg_tviews_drop('tv_post');

-- 8. Control: with 1,000 categories of 2 items the lookup column is selective
--    enough for any plan, so a scan here would be the test's, not the write's.
SELECT build(1000, 2, false);
SELECT check_write('control, 1,000 categories', $$UPDATE tb_cat SET name = 'six' WHERE pk_cat = 2$$);

SELECT 'fan-out fresh plan: PASS' AS result;
-- expect-output: fan-out fresh plan: PASS
