-- Regression test (#224): a write that changes nothing a TVIEW reads costs it
-- nothing.
--
-- The statement trigger of each mapped TVIEW ran two queries over the transition
-- tables even when the UPDATE changed none of the columns that TVIEW reads, and
-- the row trigger recomputed the TVIEW's own row after any UPDATE. Both now
-- decide from the changed rows themselves: no refresh, no value lock, no query.
-- Statements that change some rows' read columns and not others', and rows that
-- move between partitions, are still mapped.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_unchanged_reads.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, bio text, last_seen timestamptz);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text, views int DEFAULT 0);
INSERT INTO tb_user SELECT g, gen_random_uuid(), 'u' || g, 'b' || g, now() FROM generate_series(1, 5) g;
INSERT INTO tb_post SELECT g, gen_random_uuid(), 1 + g % 5, 't' || g FROM generate_series(1, 20) g;
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);

CREATE FUNCTION work() RETURNS jsonb LANGUAGE sql AS $$
    SELECT jsonb_build_object(
        'recomputes', (tviews.pg_tviews_queue_stats()->>'view_recomputes')::bigint,
        'patched', (tviews.pg_tviews_queue_stats()->>'direct_patches_applied')::bigint,
        'locks', (tviews.pg_tviews_queue_stats()->>'value_locks')::bigint) $$;
CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['user', 'post'] LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tviews.public__tv_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM tviews.public__tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d', entity) INTO d;
        IF d <> 0 THEN
            RAISE EXCEPTION 'unchanged reads FAIL after %: tv_% diverges (% rows)', step, entity, d;
        END IF;
    END LOOP;
END $$;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'unchanged reads FAIL: %', what; END IF; END $$;

-- 1. Columns no TVIEW reads: nothing at all, one row or all of them.
DO $$
DECLARE before jsonb := work();
BEGIN
    UPDATE tb_user SET last_seen = now() + interval '1 day' WHERE pk_user = 1;
    UPDATE tb_user SET last_seen = now() + interval '2 days';
    UPDATE tb_post SET views = views + 1;
    PERFORM must(work() = before, format('unread columns did work: %s -> %s', before, work()));
END $$;
-- The same value written again: nothing either.
DO $$
DECLARE before jsonb := work();
BEGIN
    UPDATE tb_user SET name = name;
    PERFORM must(work() = before, format('same values did work: %s -> %s', before, work()));
END $$;

-- 2. A read column changed in some rows of a statement and not in others.
UPDATE tb_user SET name = CASE WHEN pk_user % 2 = 0 THEN 'even' || pk_user ELSE name END,
                   last_seen = now();
SELECT assert_fresh('some rows changed');
UPDATE tb_post SET title = 'x' || pk_post WHERE pk_post <= 3;
SELECT assert_fresh('local change');

-- 3. A partitioned table whose UPDATE moves a row to another partition.
CREATE TABLE tb_region (pk_region int, id uuid NOT NULL DEFAULT gen_random_uuid(), zone int,
                        label text, PRIMARY KEY (pk_region, zone)) PARTITION BY LIST (zone);
CREATE TABLE tb_region_1 PARTITION OF tb_region FOR VALUES IN (1);
CREATE TABLE tb_region_2 PARTITION OF tb_region FOR VALUES IN (2);
CREATE TABLE tb_shop (pk_shop int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_region int);
INSERT INTO tb_region (pk_region, zone, label) VALUES (1, 1, 'north'), (2, 2, 'south');
INSERT INTO tb_shop VALUES (1, default, 1), (2, default, 2), (3, default, 1);
SELECT tviews.pg_tviews_create('tv_shop', $$
    SELECT s.pk_shop, s.id, jsonb_build_object('region', r.label, 'zone', r.zone) AS data
    FROM tb_shop s JOIN tb_region r ON r.pk_region = s.fk_region $$);
UPDATE tb_region SET zone = 2 WHERE pk_region = 1;
UPDATE tb_region SET label = 'moved' WHERE pk_region = 2;
UPDATE tb_region SET zone = 1, label = 'back' WHERE pk_region = 1;
DO $$ BEGIN
    IF EXISTS (SELECT pk_shop, data FROM tv_shop EXCEPT SELECT pk_shop, data FROM tviews.public__tv_shop) THEN
        RAISE EXCEPTION 'unchanged reads FAIL: tv_shop diverges after partition moves';
    END IF;
END $$;

SELECT 'unchanged reads: PASS' AS result;
-- expect-output: unchanged reads: PASS
