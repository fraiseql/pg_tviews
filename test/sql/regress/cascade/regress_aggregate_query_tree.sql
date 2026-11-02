-- An aggregate TVIEW's paths come from its query tree (architecture review A8).
--
-- The declared group keys named each source table by its bare name, looked up
-- among the view's tables with LIMIT 1, so two tables of one name in two schemas
-- could bind the wrong one; and the paths came from the declaration while the key
-- mappings came from the analysis, so the two could disagree unseen. The paths are
-- now the analysis's (each table's column equal to the group key, with its
-- attribute number); a declared group key the analysis does not find fails CREATE.
-- A source of the key the declaration does not name (a table read in a subquery
-- correlated on the key) keeps its path: it used to get none, and its writes left
-- the groups stale.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_aggregate_query_tree.sql
-- expect-output: aggregate_query_tree: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'A8 FAIL: %', msg; END IF; END $$;
CREATE FUNCTION refused(def text, keys jsonb) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    PERFORM tviews.pg_tviews_create('tv_bad', def, jsonb_build_object('group_keys', keys));
    PERFORM tviews.pg_tviews_drop('tv_bad');
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    RETURN SQLERRM;
END $$;

-- Two tables named tb_order, in two schemas; the view groups app.tb_order.
CREATE SCHEMA app;
CREATE SCHEMA archive;
CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
CREATE TABLE archive.tb_order (pk_order bigint PRIMARY KEY, fk_user bigint NOT NULL,
                               fk_owner bigint NOT NULL, total numeric NOT NULL);
CREATE TABLE app.tb_order (pk_order bigint PRIMARY KEY, fk_user bigint NOT NULL,
                           total numeric NOT NULL);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann'), (2, 'bob');
INSERT INTO app.tb_order VALUES (1, 1, 10), (2, 1, 20), (3, 2, 5);
INSERT INTO archive.tb_order VALUES (1, 2, 2, 99);

-- The archive is read too (its owners' totals), so both tables are base tables
-- of the view; only app.tb_order's fk_user is the group key.
SELECT tviews.pg_tviews_create('tv_user_total', $$
    SELECT o.fk_user AS pk_user_total, u.id,
           jsonb_build_object('name', u.name, 'total', sum(o.total),
                              'archived', (SELECT count(*) FROM archive.tb_order a
                                           WHERE a.fk_owner = o.fk_user)) AS data
    FROM app.tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id, u.name
$$, '{"group_keys": {"tb_order": "fk_user", "tb_user": "pk_user"}}') \g /dev/null

-- The path of the group key is app.tb_order's, with its attribute number.
SELECT must(EXISTS (SELECT 1 FROM tviews.pg_tview_meta m,
                           jsonb_array_elements(m.plan->'paths') p
                     WHERE m.entity = 'user_total'
                       AND (p->>'source_oid')::oid = 'app.tb_order'::regclass
                       AND p->>'initial_col' = 'fk_user'
                       AND (p->>'initial_attnum')::int = 2),
            'the group key is not bound to app.tb_order.fk_user with its number: '
            || (SELECT (plan->'paths')::text FROM tviews.pg_tview_meta WHERE entity = 'user_total'));
-- archive.tb_order's fk_owner equals the key too: it gets a path of its own.
SELECT must(EXISTS (SELECT 1 FROM tviews.pg_tview_meta m,
                           jsonb_array_elements(m.plan->'paths') p
                     WHERE m.entity = 'user_total'
                       AND (p->>'source_oid')::oid = 'archive.tb_order'::regclass
                       AND p->>'initial_col' = 'fk_owner'),
            'archive.tb_order.fk_owner got no path');
-- And it refreshes: a new order of bob, an order moved to ann, an archived order
-- of ann's.
INSERT INTO app.tb_order VALUES (4, 2, 7);
UPDATE app.tb_order SET fk_user = 1 WHERE pk_order = 3;
INSERT INTO archive.tb_order VALUES (2, 1, 1, 50);
SELECT must(NOT EXISTS (SELECT pk_user_total, data FROM tv_user_total
                        EXCEPT SELECT pk_user_total, data FROM tviews.public__tv_user_total)
            AND NOT EXISTS (SELECT pk_user_total, data FROM tviews.public__tv_user_total
                            EXCEPT SELECT pk_user_total, data FROM tv_user_total),
            'tv_user_total is stale');

-- A declared group key the query tree does not group by: refused.
SELECT must(refused($$
    SELECT o.fk_user AS pk_bad, u.id, jsonb_build_object('total', sum(o.total)) AS data
    FROM app.tb_order o JOIN tb_user u ON u.pk_user = o.fk_user GROUP BY o.fk_user, u.id
$$, '{"tb_order": "pk_order", "tb_user": "pk_user"}') LIKE '%group_keys%tb_order%pk_order%',
            'a group key the definition does not group by was accepted');

\echo 'aggregate_query_tree: PASS'
