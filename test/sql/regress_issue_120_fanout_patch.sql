-- Regression test (#120): a parent's changed fields are written into all its
-- children in one statement.
--
-- tv_post copies u.name into its data. An UPDATE of tb_user.name used to enqueue
-- every post of that user and recompute each from its backing view. It is now written into
-- all of them by one UPDATE keyed by fk_user, without evaluating the view. A
-- change the children do not copy unchanged still recomputes them.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_120_fanout_patch.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text,
    bio     text NOT NULL DEFAULT '',
    score   numeric NOT NULL DEFAULT 0
);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user,
    title   text NOT NULL
);
CREATE TABLE tb_feed (
    pk_feed int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post int NOT NULL REFERENCES tb_post
);
INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'alice', 'a'), (2, 'bob', 'b'), (3, 'carol', 'c');
INSERT INTO tb_post (pk_post, fk_user, title)
    SELECT g, 1 + g % 2, 'p' || g FROM generate_series(1, 20) g;
INSERT INTO tb_feed (pk_feed, fk_post) SELECT g, g FROM generate_series(1, 20) g;

SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object(
               'title', p.title,
               'author_name', u.name,
               'author_bio_len', length(u.bio),
               'author_score', u.score) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
SELECT pg_tviews_create('tv_feed', $$
    SELECT f.pk_feed, f.id, f.fk_post, jsonb_build_object('post', v.data) AS data
    FROM tb_feed f JOIN tv_post v ON v.pk_post = f.fk_post $$);

CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['post', 'feed'] LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tviews.public__tv_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM tviews.public__tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d',
            entity) INTO d;
        IF d <> 0 THEN
            RAISE EXCEPTION '#120 FAIL after %: tv_% is stale (% rows differ from v_%)',
                step, entity, d, entity;
        END IF;
    END LOOP;
END $$;

-- Counters are per transaction: read them in the same DO block as the write.
CREATE FUNCTION stat(name text) RETURNS bigint LANGUAGE sql AS
    $$ SELECT (pg_tviews_queue_stats()->>name)::bigint $$;

-- (1) A copied column: one statement patches alice's 10 posts, no post recompute
--     (the feeds embedding them are recomputed through propagation).
DO $$
DECLARE
    applied bigint := stat('direct_patches_applied');
    recomputes bigint := stat('view_recomputes');
BEGIN
    UPDATE tb_user SET name = 'alice2' WHERE pk_user = 1;
    IF stat('direct_patches_applied') - applied <> 10 THEN
        RAISE EXCEPTION '#120 FAIL: % posts patched, expected 10',
            stat('direct_patches_applied') - applied;
    END IF;
    IF stat('view_recomputes') - recomputes <> 10 THEN
        RAISE EXCEPTION '#120 FAIL: % recomputes, expected the 10 feeds only',
            stat('view_recomputes') - recomputes;
    END IF;
END $$;
SELECT assert_fresh('name update');

-- (2) NULL is written as a JSON null, like jsonb_build_object does.
UPDATE tb_user SET name = NULL WHERE pk_user = 2;
SELECT assert_fresh('name set to NULL');
UPDATE tb_user SET name = 'bob2' WHERE pk_user = 2;
SELECT assert_fresh('name back from NULL');

-- (3) A column used in an expression recomputes.
UPDATE tb_user SET bio = 'longer bio' WHERE pk_user = 1;
SELECT assert_fresh('bio update (expression)');

-- (4) A copied column of a type the patch cannot render recomputes.
UPDATE tb_user SET score = 1.50 WHERE pk_user = 1;
SELECT assert_fresh('numeric update');

-- (5) A copied and an uncopied column together recompute.
UPDATE tb_user SET name = 'alice3', bio = 'x' WHERE pk_user = 1;
SELECT assert_fresh('name + bio update');

-- (6) Several parents in one statement, a parent without children, a no-op.
UPDATE tb_user SET name = name || '!';
SELECT assert_fresh('multi-row update');
UPDATE tb_user SET name = name WHERE pk_user = 1;
SELECT assert_fresh('no-op update');

-- (7) Explicit transaction, rolled-back savepoint.
BEGIN;
UPDATE tb_user SET name = 'kept' WHERE pk_user = 1;
SAVEPOINT s;
UPDATE tb_user SET name = 'rolled back' WHERE pk_user = 1;
ROLLBACK TO SAVEPOINT s;
UPDATE tb_user SET name = 'bob3' WHERE pk_user = 2;
COMMIT;
SELECT assert_fresh('transaction with a rolled-back savepoint');
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_post WHERE data->>'author_name' = 'rolled back') THEN
        RAISE EXCEPTION '#120 FAIL: a rolled-back name reached tv_post';
    END IF;
END $$;

-- (8) The patched children are reported by pg_tviews_flush_and_report.
DO $$
DECLARE rep jsonb;
BEGIN
    UPDATE tb_user SET name = 'reported' WHERE pk_user = 1;
    rep := pg_tviews_flush_and_report();
    IF (SELECT count(*) FROM jsonb_array_elements(rep->'updated') e
        WHERE e->>'__typename' = 'Post' AND e->'data'->>'author_name' = 'reported') <> 10 THEN
        RAISE EXCEPTION '#120 FAIL: report lists % patched posts', rep;
    END IF;
END $$;
SELECT assert_fresh('flush_and_report');

-- (9) With pg_tviews.direct_patch_enabled off every child recomputes.
SET pg_tviews.direct_patch_enabled = off;
DO $$
DECLARE applied bigint := stat('direct_patches_applied');
BEGIN
    UPDATE tb_user SET name = 'recomputed' WHERE pk_user = 1;
    IF stat('direct_patches_applied') <> applied THEN
        RAISE EXCEPTION '#120 FAIL: patched with direct_patch_enabled off';
    END IF;
END $$;
RESET pg_tviews.direct_patch_enabled;
SELECT assert_fresh('direct_patch_enabled off');

-- The recorded fan-out: copied name, not bio (expression) or pk_user (the key).
DO $$
DECLARE fanout jsonb;
BEGIN
    -- Recorded with the table's key mapping (ADR 0157).
    SELECT e->'fanout' INTO fanout
    FROM pg_tview_meta, jsonb_array_elements(key_mappings) e
    WHERE entity = 'post' AND (e->>'relid')::oid = 'tb_user'::regclass::oid;
    IF fanout IS DISTINCT FROM
       '{"lookup_col": "fk_user", "fields": [["name", "author_name"], ["score", "author_score"]]}' THEN
        RAISE EXCEPTION '#120 FAIL: recorded fan-out is %', fanout;
    END IF;
END $$;

SELECT 'issue #120 fan-out patch: PASS' AS result;
-- expect-output: issue #120 fan-out patch: PASS
