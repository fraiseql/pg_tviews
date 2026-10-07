-- Fixture TVIEWs for the upgrade checks (test/upgrade/upgrade_check.sh). Created with
-- the PREVIOUS release, so only use what every supported release offers: unqualified
-- pg_tviews_create / pg_tviews_create_aggregate on a search_path that reaches them
-- (schema tviews since 0.1.0-beta.20, public before).

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
SET search_path TO "$user", public, tviews;

CREATE TABLE tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    bio     text
);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user,
    title   text NOT NULL
);
CREATE TABLE tb_comment (
    pk_comment int PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post    int NOT NULL REFERENCES tb_post,
    body       text NOT NULL
);
CREATE TABLE tb_order (
    pk_order int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user  int NOT NULL REFERENCES tb_user,
    total    numeric NOT NULL
);
CREATE SCHEMA app;
CREATE TABLE app.tb_note (
    pk_note int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    body    text NOT NULL
);

INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'alice', 'a'), (2, 'bob', 'b');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2'), (3, 2, 'p3');
INSERT INTO tb_comment (pk_comment, fk_post, body) VALUES (1, 1, 'c1'), (2, 1, 'c2');
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (1, 1, 10), (2, 2, 5);
INSERT INTO app.tb_note (pk_note, body) VALUES (1, 'n1');
-- Versioned rows: one TVIEW row per id_contract (DISTINCT ON).
CREATE TABLE tb_contract (
    pk_contract int PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    id_contract int NOT NULL,
    version_no  int NOT NULL,
    status      text NOT NULL
);
INSERT INTO tb_contract (pk_contract, id_contract, version_no, status)
    VALUES (1, 100, 1, 'a'), (2, 100, 2, 'b'), (3, 200, 1, 'a');
CREATE TABLE tb_shipment (
    pk_shipment int PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    code        text NOT NULL UNIQUE,
    fk_order    int NOT NULL REFERENCES tb_order
);
INSERT INTO tb_shipment (pk_shipment, code, fk_order) VALUES (1, 's1', 1), (2, 's2', 2);

-- Plain.
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user $$);
-- Cascading (embeds another TVIEW) and array (aggregates children).
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object(
               'title', p.title,
               'author', u.data,
               'comments', COALESCE(jsonb_agg(jsonb_build_object('body', c.body)
                   ORDER BY c.pk_comment) FILTER (WHERE c.pk_comment IS NOT NULL),
                   '[]'::jsonb)) AS data
    FROM tb_post p
    JOIN tv_user u ON u.pk_user = p.fk_user
    LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
    GROUP BY p.pk_post, p.id, p.fk_user, p.title, u.data $$);
-- Two hops: tb_user -> tb_post -> tb_comment.
SELECT pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('body', c.body, 'author', u.name) AS data
    FROM tb_comment c
    JOIN tb_post p ON p.pk_post = c.fk_post
    JOIN tb_user u ON u.pk_user = p.fk_user $$);
-- Aggregate.
SELECT pg_tviews_create_aggregate('tv_user_orders', $$
    SELECT o.fk_user AS pk_user_orders, u.id,
           jsonb_build_object('orders', count(*), 'total', sum(o.total)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');
-- DISTINCT ON TVIEWs, from 0.1.0-beta.22 (which maps tables read through their
-- joins, #164) on.
SELECT COALESCE(NULLIF(pg_catalog.split_part(extversion, 'beta.', 2), '')::int >= 22, false)
       AS distinct_on_fixtures
FROM pg_catalog.pg_extension WHERE extname = 'pg_tviews' \gset
\if :distinct_on_fixtures
-- DISTINCT ON the key, aliased as pk_<entity>.
SELECT pg_tviews_create('tv_contract', $$
    SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, c.id,
           jsonb_build_object('status', c.status) AS data
    FROM tb_contract c ORDER BY c.id_contract, c.version_no DESC $$);
-- DISTINCT ON a unique root column, a table read through a join (0.1.0-beta.22
-- keys it with a unique index on pk_<entity>).
SELECT pg_tviews_create('tv_shipment', $$
    SELECT DISTINCT ON (s.code) s.pk_shipment, s.id, s.code,
           jsonb_build_object('code', s.code, 'total', o.total) AS data
    FROM tb_shipment s JOIN tb_order o ON o.pk_order = s.fk_order ORDER BY s.code $$);
-- The ancestors of a node through a path of ids, unnested in a subquery: created
-- with tb_node all_keys before 0.1.0-beta.25 (#182), mapped once re-registered.
CREATE TABLE tb_node (pk_node int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
    path text NOT NULL, name text NOT NULL);
INSERT INTO tb_node (pk_node, path, name) VALUES (1, '1', 'root'), (2, '1.2', 'child'), (3, '1.2.3', 'leaf');
SELECT pg_tviews_create('tv_node', $$
    SELECT s.pk_node, s.id, jsonb_build_object('up', array_agg(a.name ORDER BY a.pk_node)) AS data
    FROM (SELECT n.pk_node, n.id, unnest(string_to_array(n.path, '.')::int[]) AS node_id
          FROM tb_node n) s
    JOIN tb_node a ON a.pk_node = s.node_id
    GROUP BY s.pk_node, s.id $$);
-- A table name long enough that its backing view's name is fitted to 63 bytes.
CREATE TABLE tb_long_entity_name_for_the_upgrade_fitter_check_abcdefghij (pk_long_entity_name_for_the_upgrade_fitter_check_abcdefghij int PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(), label text NOT NULL);
INSERT INTO tb_long_entity_name_for_the_upgrade_fitter_check_abcdefghij (pk_long_entity_name_for_the_upgrade_fitter_check_abcdefghij, label) VALUES (1, 'l1');
SELECT pg_tviews_create('tv_long_entity_name_for_the_upgrade_fitter_check_abcdefghij', $$
    SELECT pk_long_entity_name_for_the_upgrade_fitter_check_abcdefghij, id, jsonb_build_object('label', label) AS data
    FROM tb_long_entity_name_for_the_upgrade_fitter_check_abcdefghij $$);
\endif
-- A virtual generated column (PostgreSQL 18) read through a join (#179).
SELECT current_setting('server_version_num')::int >= 180000 AS virtual_fixture \gset
\if :virtual_fixture
CREATE TABLE tb_badge (pk_badge int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
    name text NOT NULL, label text GENERATED ALWAYS AS (upper(name)));
CREATE TABLE tb_holder (pk_holder int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_badge int NOT NULL REFERENCES tb_badge);
INSERT INTO tb_badge (pk_badge, name) VALUES (1, 'b1');
INSERT INTO tb_holder (pk_holder, fk_badge) VALUES (1, 1), (2, 1);
SELECT pg_tviews_create('tv_holder', $$
    SELECT h.pk_holder, h.id, jsonb_build_object('badge', b.label) AS data
    FROM tb_holder h JOIN tb_badge b ON b.pk_badge = h.fk_badge $$);
\endif
-- A TVIEW dropped with its schema while its base table lives elsewhere: from
-- 0.1.0-beta.25 its backing view was left in tviews (#186); the upgrade drops it.
-- Schema-qualified TVIEW names need the tviews schema (0.1.0 has neither).
SELECT pg_catalog.to_regclass('tviews.registry') IS NOT NULL AS scratch_fixture \gset
\if :scratch_fixture
CREATE TABLE tb_scratch (pk_scratch int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), label text);
CREATE SCHEMA scratch;
SELECT pg_tviews_create('scratch.tv_scratch', $$
    SELECT pk_scratch, id, jsonb_build_object('label', label) AS data FROM public.tb_scratch $$);
DROP SCHEMA scratch CASCADE;
\endif
-- Off the search_path.
SET search_path TO app, public, tviews;
SELECT pg_tviews_create('tv_note', $$
    SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM app.tb_note $$);
SET search_path TO "$user", public, tviews;

-- Readers of the TVIEWs, for the backing views' privileges (#181): one granted
-- every table of public at once, which reached the backing views while they were
-- public.v_<entity>; one granted tv_user alone.
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'upgrade_schema_reader') THEN
        CREATE ROLE upgrade_schema_reader;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'upgrade_table_reader') THEN
        CREATE ROLE upgrade_table_reader;
    END IF;
END $$;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO upgrade_schema_reader;
GRANT SELECT ON tv_user TO upgrade_table_reader;
