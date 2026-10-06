-- ADR 0157, part B: every base table a TVIEW reads is classified from the
-- backing view's query tree, not from its SQL text.
--
--   local       the key is a column of the changed row (the root table, a table
--               linked by `col = <key>`, also inside a subquery or a view)
--   mapped      a chain of conditions links the table to the key
--   propagated  read through the view of a TVIEW this one embeds (fk_<entity>)
--   all_keys    nothing selective links it to the key
--
-- tviews.registry.cascade_kinds shows the classification; the set of tables equals
-- what pg_depend reports (registry.base_tables).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_adr_0157_classification.sql
--
-- expect-output: tv_fn calls public.label_of(), which is not immutable
-- expect-output: ADR 0157 classification: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
-- Tables no cascade reaches are what this file classifies: the TVIEWs accept them.
SET pg_tviews.uncascaded_policy = 'warn';

CREATE TABLE tb_user  (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_user bigint REFERENCES tb_user, ref text, min_pos int NOT NULL DEFAULT 0);
CREATE TABLE tb_sku   (pk_sku bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_line  (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_order bigint NOT NULL REFERENCES tb_order, fk_sku bigint REFERENCES tb_sku,
                       pos int NOT NULL, sku text);
CREATE TABLE tb_flag  (pk_flag bigint PRIMARY KEY, active boolean);
CREATE TABLE tb_book  (pk_book bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_movie (pk_movie bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_user VALUES (1, DEFAULT, 'u');
INSERT INTO tb_order VALUES (1, DEFAULT, 1, 'o1', 0);
INSERT INTO tb_sku VALUES (1, DEFAULT, 's');
INSERT INTO tb_line VALUES (1, DEFAULT, 1, 1, 1, 'a');
INSERT INTO tb_book VALUES (1, DEFAULT, 'b');
INSERT INTO tb_movie VALUES (2, DEFAULT, 'm');

-- One root table per TVIEW (pk_<entity> over tb_<entity>), joined to tb_line on
-- the same key values.
DO $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY ARRAY['basket', 'late', 'buyer', 'invoice', 'lat', 'cte', 'rank', 'fn'] LOOP
        EXECUTE format('CREATE TABLE tb_%1$s (pk_%1$s bigint PRIMARY KEY, '
                       'id uuid NOT NULL DEFAULT gen_random_uuid(), name text, '
                       'min_pos int NOT NULL DEFAULT 0)', e);
        EXECUTE format('INSERT INTO tb_%s VALUES (1, DEFAULT, ''x'', 0)', e);
    END LOOP;
END $$;

CREATE VIEW v_order_lines AS
    SELECT l.fk_order, jsonb_agg(l.sku ORDER BY l.pos) AS lines FROM tb_line l GROUP BY l.fk_order;
CREATE VIEW v_order_lines_2 AS SELECT * FROM v_order_lines;
CREATE FUNCTION label_of(t text) RETURNS text LANGUAGE sql STABLE AS $$ SELECT upper(t) $$;

-- The expected kind of every base table of every TVIEW.
CREATE TABLE expected (entity text, tbl text, kind text);

-- root and a direct fk
SELECT pg_tviews_create('tv_user', $$ SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
INSERT INTO expected VALUES ('user', 'tb_user', 'local');
SELECT pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id, o.fk_user,
           jsonb_build_object('ref', o.ref, 'user', u.data,
                              'skus', COALESCE(jsonb_agg(s.name) FILTER (WHERE s.pk_sku IS NOT NULL), '[]')) AS data
    FROM tb_order o
    JOIN tviews.public__tv_user u ON u.pk_user = o.fk_user
    LEFT JOIN tb_line l ON l.fk_order = o.pk_order
    LEFT JOIN tb_sku s ON s.pk_sku = l.fk_sku
    GROUP BY o.pk_order, o.id, o.fk_user, o.ref, u.data $$);
INSERT INTO expected VALUES ('order', 'tb_order', 'local'), ('order', 'tb_line', 'local'),
                            ('order', 'tb_sku', 'mapped'), ('order', 'tb_user', 'propagated');

-- #157: correlated subqueries in the select list
SELECT pg_tviews_create_or_replace('tv_basket', $$
    SELECT o.pk_basket, o.id,
           ARRAY(SELECT l.sku FROM tb_line l WHERE l.fk_order = o.pk_basket ORDER BY l.pos) AS skus,
           jsonb_build_object(
               'skun', (SELECT string_agg(s.name, ',') FROM tb_line l2 JOIN tb_sku s ON s.pk_sku = l2.fk_sku
                        WHERE l2.fk_order = o.pk_basket),
               'flags', (SELECT count(*) FROM tb_flag)) AS data
    FROM tb_basket o $$, '{"uncascaded_policy": "warn"}');
INSERT INTO expected VALUES ('basket', 'tb_basket', 'local'), ('basket', 'tb_line', 'local'),
                            ('basket', 'tb_sku', 'mapped'), ('basket', 'tb_flag', 'all_keys');

-- EXISTS with a non-equality correlation; IN (SELECT …)
SELECT pg_tviews_create('tv_late', $$
    SELECT o.pk_late, o.id,
           jsonb_build_object('late', EXISTS (SELECT 1 FROM tb_line l WHERE l.pos > o.min_pos)) AS data
    FROM tb_late o $$);
INSERT INTO expected VALUES ('late', 'tb_late', 'local'), ('late', 'tb_line', 'mapped');
SELECT pg_tviews_create('tv_buyer', $$
    SELECT u.pk_buyer, u.id, jsonb_build_object('name', u.name) AS data
    FROM tb_buyer u WHERE u.pk_buyer IN (SELECT o.fk_user FROM tb_order o WHERE o.ref IS NOT NULL) $$);
INSERT INTO expected VALUES ('buyer', 'tb_buyer', 'local'), ('buyer', 'tb_order', 'local');

-- #158: a view with GROUP BY, and a view over it
SELECT pg_tviews_create('tv_invoice', $$
    SELECT o.pk_invoice, o.id, jsonb_build_object('lines', v.lines) AS data
    FROM tb_invoice o LEFT JOIN v_order_lines_2 v ON v.fk_order = o.pk_invoice $$);
INSERT INTO expected VALUES ('invoice', 'tb_invoice', 'local'), ('invoice', 'tb_line', 'local');

-- LATERAL; a CTE; a window function without PARTITION BY
SELECT pg_tviews_create('tv_lat', $$
    SELECT o.pk_lat, o.id, jsonb_build_object('first', f.sku) AS data
    FROM tb_lat o
    LEFT JOIN LATERAL (SELECT l.sku FROM tb_line l WHERE l.fk_order = o.pk_lat ORDER BY l.pos LIMIT 1) f ON true $$);
INSERT INTO expected VALUES ('lat', 'tb_lat', 'local'), ('lat', 'tb_line', 'local');
SELECT pg_tviews_create('tv_cte', $$
    WITH lines AS (SELECT l.fk_order, l.fk_sku FROM tb_line l)
    SELECT o.pk_cte, o.id, jsonb_build_object('n', count(s.pk_sku)) AS data
    FROM tb_cte o LEFT JOIN lines x ON x.fk_order = o.pk_cte LEFT JOIN tb_sku s ON s.pk_sku = x.fk_sku
    GROUP BY o.pk_cte, o.id $$);
INSERT INTO expected VALUES ('cte', 'tb_cte', 'local'), ('cte', 'tb_line', 'local'), ('cte', 'tb_sku', 'mapped');
SELECT pg_tviews_create('tv_rank', $$
    SELECT o.pk_rank, o.id, jsonb_build_object('rank', r.rn) AS data
    FROM tb_rank o
    LEFT JOIN (SELECT l.fk_order, row_number() OVER (ORDER BY l.pos) AS rn FROM tb_line l) r
           ON r.fk_order = o.pk_rank $$);
INSERT INTO expected VALUES ('rank', 'tb_rank', 'local'), ('rank', 'tb_line', 'all_keys');

-- UNION branches: each branch's table is the root of its branch
SELECT pg_tviews_create('tv_media', $$
    SELECT b.pk_book AS pk_media, b.id, jsonb_build_object('title', b.title) AS data FROM tb_book b
    UNION ALL
    SELECT m.pk_movie AS pk_media, m.id, jsonb_build_object('title', m.title) AS data FROM tb_movie m $$);
INSERT INTO expected VALUES ('media', 'tb_book', 'local'), ('media', 'tb_movie', 'local');

-- an aggregate TVIEW: the key is the group key
SELECT pg_tviews_create_aggregate('tv_user_orders', $$
    SELECT o.fk_user AS pk_user_orders, u.id, jsonb_build_object('orders', count(*)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');
INSERT INTO expected VALUES ('user_orders', 'tb_order', 'local'), ('user_orders', 'tb_user', 'local');

-- a function that may read tables pg_tviews cannot see
SET client_min_messages TO NOTICE;
SELECT pg_tviews_create('tv_fn', $$
    SELECT u.pk_fn, u.id, jsonb_build_object('label', label_of(u.name)) AS data FROM tb_fn u $$);
SET client_min_messages TO WARNING;
INSERT INTO expected VALUES ('fn', 'tb_fn', 'local');

-- Compare: same tables as pg_depend, and the expected kind for each.
DO $$
DECLARE r record; bad text := '';
BEGIN
    FOR r IN
        SELECT coalesce(e.entity, k.entity) AS entity, coalesce(e.tbl, k.tbl) AS tbl,
               e.kind AS want, k.kind AS got
        FROM expected e
        FULL JOIN (SELECT g.entity, c.key AS tbl, c.value AS kind
                   FROM tviews.registry g, jsonb_each_text(g.cascade_kinds) c) k
          ON k.entity = e.entity AND k.tbl = e.tbl
        WHERE e.kind IS DISTINCT FROM k.kind
    LOOP
        bad := bad || format(E'\n  %s.%s: want %s, got %s', r.entity, r.tbl, r.want, r.got);
    END LOOP;
    IF bad <> '' THEN
        RAISE EXCEPTION 'FAIL ADR 0157 classification:%', bad;
    END IF;
    IF EXISTS (SELECT 1 FROM tviews.registry g
               WHERE (SELECT array_agg(k ORDER BY k) FROM jsonb_object_keys(g.cascade_kinds) k)
                     IS DISTINCT FROM (SELECT array_agg(b::text ORDER BY b::text) FROM unnest(g.base_tables) b))
    THEN
        RAISE EXCEPTION 'FAIL ADR 0157: cascade_kinds and base_tables list different tables';
    END IF;
END $$;

\echo 'ADR 0157 classification: PASS'
