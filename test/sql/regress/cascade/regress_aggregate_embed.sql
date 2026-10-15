-- Regression test (#126): a TVIEW embedding an aggregate TVIEW follows it.
--
-- Propagation finds parent rows by the parent's fk_<child> column. An aggregate
-- TVIEW (#58) is keyed by its group key and no parent has an fk_<aggregate>
-- column, so a TVIEW embedding v_<aggregate>.data was never refreshed when the
-- aggregate changed. The create now records the output column joined to
-- pk_<aggregate> and propagation looks parent rows up by it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_aggregate_embed.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user BIGINT PRIMARY KEY,
    id      UUID NOT NULL DEFAULT gen_random_uuid(),
    name    TEXT NOT NULL
);
CREATE TABLE tb_order (
    pk_order BIGINT PRIMARY KEY,
    id       UUID NOT NULL DEFAULT gen_random_uuid(),
    fk_user  BIGINT NOT NULL REFERENCES tb_user,
    total    NUMERIC NOT NULL
);
CREATE TABLE tb_post (
    pk_post BIGINT PRIMARY KEY,
    id      UUID NOT NULL DEFAULT gen_random_uuid(),
    fk_user BIGINT NOT NULL REFERENCES tb_user,
    title   TEXT NOT NULL
);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob'), (3, 'carol');
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (1, 1, 10), (2, 1, 20), (3, 2, 5);
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2'), (3, 3, 'p3');

SELECT pg_tviews_create('tv_user_summary', $$
    SELECT o.fk_user AS pk_user_summary, u.id,
           jsonb_build_object('orders', count(*), 'total', sum(o.total)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"group_keys": {"tb_order": "fk_user", "tb_user": "pk_user"}}');

-- Embedded through the parent's own key, under a LEFT JOIN (users without orders).
SELECT pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id,
           jsonb_build_object('name', u.name, 'summary', s.data) AS data
    FROM tb_user u LEFT JOIN tv_user_summary s ON s.pk_user_summary = u.pk_user
$$);
-- Embedded through another projected column, under an alias.
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user AS author_pk,
           jsonb_build_object('title', p.title, 'author_summary', s.data) AS data
    FROM tb_post p LEFT JOIN tv_user_summary s ON p.fk_user = s.pk_user_summary
$$);

CREATE FUNCTION must(ok BOOLEAN, msg TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION '#126 FAIL: %', msg; END IF; END $$;

CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['user_summary', 'user', 'post'] LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tviews.public__tv_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM tviews.public__tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d',
            entity) INTO d;
        PERFORM must(d = 0, format('after %s: tv_%s is stale (%s rows differ from tviews.public__tv_%s)',
                                   step, entity, d, entity));
    END LOOP;
END $$;

SELECT assert_fresh('create');

-- A changed group reaches the rows embedding it.
UPDATE tb_order SET total = 99 WHERE pk_order = 1;
SELECT assert_fresh('order total update');
SELECT must((SELECT data->'summary'->>'total' FROM tv_user WHERE pk_user = 1) = '119',
            'tv_user 1 shows the new total');

-- A new group (carol's first order) appears under the LEFT JOIN.
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (4, 3, 7);
SELECT assert_fresh('first order of a user');
SELECT must((SELECT data->'author_summary'->>'orders' FROM tv_post WHERE pk_post = 3) = '1',
            'tv_post 3 shows carol''s new summary');

-- An order moving between users refreshes both groups and both embedding rows.
UPDATE tb_order SET fk_user = 2 WHERE pk_order = 2;
SELECT assert_fresh('order moved between users');

-- An emptied group disappears from the embedding rows.
DELETE FROM tb_order WHERE pk_order = 4;
SELECT assert_fresh('last order of a user deleted');
SELECT must((SELECT data->'summary' FROM tv_user WHERE pk_user = 3) = 'null',
            'tv_user 3 has no summary again');

-- Several groups in one statement (bulk refresh, batched propagation).
UPDATE tb_order SET total = total + 1;
SELECT assert_fresh('multi-row order update');

-- Explicit transaction: flushed at COMMIT.
BEGIN;
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (5, 3, 1), (6, 3, 2);
UPDATE tb_user SET name = 'carol2' WHERE pk_user = 3;
COMMIT;
SELECT assert_fresh('explicit transaction');

-- A definition reading an aggregate without projecting the column joined to its
-- key is rejected: nothing could route a group change to its rows.
CREATE TABLE tb_note (
    pk_note BIGINT PRIMARY KEY,
    id      UUID NOT NULL DEFAULT gen_random_uuid(),
    fk_user BIGINT NOT NULL REFERENCES tb_user
);
DO $$
BEGIN
    PERFORM pg_tviews_create('tv_note', $v$
        SELECT n.pk_note, n.id, s.data
        FROM tb_note n JOIN tv_user_summary s ON s.pk_user_summary = n.fk_user
    $v$);
    RAISE EXCEPTION '#126 FAIL: tv_note was created';
EXCEPTION WHEN OTHERS THEN
    IF SQLERRM NOT LIKE '%reads aggregate TVIEW ''user_summary''%' THEN
        RAISE;
    END IF;
END $$;
SELECT must(to_regclass('tv_note') IS NULL, 'rejected create left no tv_note');

-- The recorded lookup columns, and an index for the one that is not the pk.
SELECT must((SELECT (SELECT jsonb_object_agg(e->>'entity', e->'lookups') FROM jsonb_array_elements(plan->'embeds') e) FROM pg_tview_meta WHERE entity = 'user')
            = '{"user_summary": ["pk_user"]}', 'tv_user records its lookup column');
SELECT must((SELECT (SELECT jsonb_object_agg(e->>'entity', e->'lookups') FROM jsonb_array_elements(plan->'embeds') e) FROM pg_tview_meta WHERE entity = 'post')
            = '{"user_summary": ["author_pk"]}', 'tv_post records its aliased lookup column');
SELECT must(to_regclass('idx_tv_post_author_pk_pk_post') IS NOT NULL,
            'tv_post has a propagation index on its lookup column');

SELECT 'issue #126 aggregate embed: PASS' AS result;
-- expect-output: issue #126 aggregate embed: PASS
