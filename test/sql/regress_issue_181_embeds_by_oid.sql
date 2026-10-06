-- Regression test for issue #181 (prerequisite): a TVIEW embedding an aggregate
-- TVIEW is found from the query tree, by OID, whatever the aggregate's backing
-- view is called. The text match on `v_<aggregate>` / `tv_<aggregate>` missed a
-- backing view that is not named v_<aggregate>.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_181_embeds_by_oid.sql
--
-- expect-output: issue #181 embeds by OID: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_user bigint NOT NULL REFERENCES tb_user, total numeric NOT NULL);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (1, 1, 10), (2, 2, 5);
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');

SELECT pg_tviews_create_aggregate('tv_user_summary', $$
    SELECT o.fk_user AS pk_user_summary, u.id,
           jsonb_build_object('orders', count(*), 'total', sum(o.total)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');

-- The backing view, whatever its name, read by the dependent definition.
SELECT format('%s', view) AS summary_view FROM tviews.registry WHERE entity = 'user_summary' \gset
ALTER VIEW :summary_view RENAME TO user_summary_def;
SELECT pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name, 'summary', s.data) AS data
    FROM tb_user u LEFT JOIN user_summary_def s ON s.pk_user_summary = u.pk_user $$);
-- The aggregate's table, read in a correlated subquery.
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title,
                              'orders', (SELECT s.data->'orders' FROM tv_user_summary s
                                         WHERE s.pk_user_summary = p.fk_user)) AS data
    FROM tb_post p $$);

CREATE FUNCTION check_fresh(e text, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE d bigint;
BEGIN
    EXECUTE format(
        'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s EXCEPT SELECT pk_%1$s, data FROM %2$s)
                     UNION ALL (SELECT pk_%1$s, data FROM %2$s EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d',
        e, (SELECT view FROM tviews.registry WHERE entity = e)) INTO d;
    IF d <> 0 THEN
        RAISE EXCEPTION '#181 FAIL: tv_% stale after %', e, label;
    END IF;
END $$;

UPDATE tb_order SET total = 99 WHERE pk_order = 1;
SELECT check_fresh('user', 'an order update'), check_fresh('post', 'an order update');
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (3, 2, 1);
SELECT check_fresh('user', 'an order insert'), check_fresh('post', 'an order insert');

DO $$ BEGIN
    IF (SELECT aggregate_embeds FROM tviews.pg_tview_meta WHERE entity = 'user')
       IS DISTINCT FROM '{"user_summary": "pk_user"}' THEN
        RAISE EXCEPTION '#181 FAIL: the embed through a renamed backing view is %',
            (SELECT aggregate_embeds FROM tviews.pg_tview_meta WHERE entity = 'user');
    END IF;
    IF (SELECT aggregate_embeds FROM tviews.pg_tview_meta WHERE entity = 'post')
       IS DISTINCT FROM '{"user_summary": "fk_user"}' THEN
        RAISE EXCEPTION '#181 FAIL: the embed in a correlated subquery is %',
            (SELECT aggregate_embeds FROM tviews.pg_tview_meta WHERE entity = 'post');
    END IF;
END $$;

-- Read with no output carrying the key: refused, naming the aggregate.
CREATE TABLE tb_note (pk_note bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user);
DO $$ BEGIN
    PERFORM pg_tviews_create('tv_note', $v$
        SELECT n.pk_note, n.id, s.data FROM tb_note n JOIN user_summary_def s ON s.pk_user_summary = n.fk_user $v$);
    RAISE EXCEPTION '#181 FAIL: tv_note was created';
EXCEPTION WHEN OTHERS THEN
    IF SQLERRM NOT LIKE '%reads aggregate TVIEW ''user_summary''%' THEN
        RAISE EXCEPTION '#181 FAIL: tv_note refused for another reason: %', SQLERRM;
    END IF;
END $$;

\echo 'issue #181 embeds by OID: PASS'
