-- Differential harness: one TVIEW per shape pg_tviews maintains, a seeded
-- generator of writes, and a check that every TVIEW equals its backing view after
-- each statement. Loaded by run.sh into a fresh database.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
\ir ../lib/assert_fresh.sql

-- Generated columns are virtual on PostgreSQL 18 (its default), STORED before.
SELECT CASE WHEN current_setting('server_version_num')::int >= 180000 THEN 'VIRTUAL'
            ELSE 'STORED' END AS generated \gset

-- No foreign keys: the generator may orphan rows, which the views must handle.
CREATE TABLE tb_customer (pk_customer int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(), name text,
  name_up text GENERATED ALWAYS AS (upper(name)) :generated);
CREATE TABLE tb_order (pk_order int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(), fk_customer int, ref text, status text);
CREATE TABLE tb_line (pk_line int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), fk_order int, sku text, qty int);
CREATE TABLE tb_contract (pk_contract int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), id_contract int NOT NULL,
  version_no int NOT NULL, status text);
CREATE TABLE tb_deal (pk_deal int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), fk_contract int, name text);
CREATE TABLE tb_desk (pk_desk int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), fk_deal int, label text);
CREATE TABLE tb_doc (pk_doc int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL, rev int NOT NULL, body text);
CREATE TABLE tb_note (pk_note int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), fk_doc int, text text);
CREATE TABLE tb_vtag (pk_vtag int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), raw text,
  norm text GENERATED ALWAYS AS (lower(raw)) :generated);
CREATE TABLE tb_vnote (pk_vnote int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), tag text, fk_customer int);
CREATE TABLE tb_task (pk_task int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_task_archive (pk_task int GENERATED ALWAYS AS IDENTITY (START 100000) PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
-- A hierarchy stored as a path of ids (#182), badges joined on a computed name.
CREATE TABLE tb_node (pk_node int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  path text NOT NULL, name text, deleted_at timestamptz);
CREATE TABLE tb_badge (pk_badge int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, code text, label text);
-- A category tree read through a recursive view (#183).
CREATE TABLE tb_category (pk_category int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  fk_parent int, name text);
CREATE TABLE tb_item (pk_item int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), fk_category int, name text);

INSERT INTO tb_customer (name) SELECT 'c' || g FROM generate_series(1, 6) g;
INSERT INTO tb_order (fk_customer, ref, status) SELECT 1 + g % 6, 'r' || g, 'new' FROM generate_series(1, 10) g;
INSERT INTO tb_line (fk_order, sku, qty) SELECT 1 + g % 10, 's' || g, g FROM generate_series(1, 20) g;
INSERT INTO tb_contract (id_contract, version_no, status)
  SELECT 100 * (1 + g % 5), 1 + g / 5, 'v' || g FROM generate_series(0, 11) g;
INSERT INTO tb_deal (fk_contract, name) SELECT 100 * (1 + g % 5), 'd' || g FROM generate_series(1, 6) g;
INSERT INTO tb_desk (fk_deal, label) SELECT 1 + g % 6, 'k' || g FROM generate_series(1, 4) g;
INSERT INTO tb_doc (id, rev, body)
  SELECT ('00000000-0000-0000-0000-00000000000' || (1 + g % 4))::uuid, 1 + g / 4, 'b' || g
  FROM generate_series(0, 7) g;
INSERT INTO tb_note (fk_doc, text) SELECT 1 + g % 8, 'n' || g FROM generate_series(1, 6) g;
INSERT INTO tb_vtag (raw) SELECT chr(65 + g) FROM generate_series(0, 3) g;
INSERT INTO tb_vnote (tag, fk_customer) SELECT chr(97 + g % 5), 1 + g % 6 FROM generate_series(0, 7) g;
INSERT INTO tb_task (title) SELECT 't' || g FROM generate_series(1, 4) g;
INSERT INTO tb_task_archive (title) SELECT 'a' || g FROM generate_series(1, 3) g;
INSERT INTO tb_node (pk_node, path, name) VALUES (1, '1', 'n1'), (2, '1.2', 'n2'), (3, '1.2.3', 'n3'),
  (4, '1.4', 'n4'), (5, '1.4.5', 'n5'), (6, '1.2.6', 'n6');
INSERT INTO tb_badge (code, label) SELECT 'N' || g, 'b' || g FROM generate_series(1, 4) g;
INSERT INTO tb_category (pk_category, fk_parent, name) VALUES (1, NULL, 'k1'), (2, 1, 'k2'), (3, 2, 'k3'),
  (4, 1, 'k4');
INSERT INTO tb_item (fk_category, name) SELECT 1 + g % 5, 'i' || g FROM generate_series(1, 6) g;
CREATE VIEW v_cnt AS SELECT fk_order, count(*) AS n, sum(qty) AS qty FROM tb_line GROUP BY fk_order;
CREATE VIEW v_category_path AS
  WITH RECURSIVE p AS (
    SELECT pk_category, ARRAY[name] AS names FROM tb_category WHERE fk_parent IS NULL
    UNION ALL
    SELECT c.pk_category, p.names || c.name FROM tb_category c JOIN p ON p.pk_category = c.fk_parent)
  SELECT pk_category, names FROM p;

-- ── shapes ───────────────────────────────────────────────────────────────────
-- A shape that cannot be created is skipped when it is listed in harness.xfail
-- (an open defect), and fails the run otherwise.
SET harness.xfail = :'xfail';
CREATE TABLE harness_shape (tv regclass PRIMARY KEY, key text NOT NULL);
CREATE FUNCTION harness_create(tv text, key text, def text, group_keys jsonb DEFAULT NULL,
                               policy text DEFAULT 'warn')
RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
    PERFORM set_config('pg_tviews.uncascaded_policy', policy, true);
    BEGIN
        IF group_keys IS NULL THEN
            PERFORM pg_tviews_create(tv, def);
        ELSE
            PERFORM pg_tviews_create_aggregate(tv, def, group_keys);
        END IF;
    EXCEPTION WHEN OTHERS THEN
        IF tv = ANY (string_to_array(current_setting('harness.xfail'), ',')) THEN
            RAISE WARNING 'XFAIL % not created: %', tv, SQLERRM;
            RETURN;
        END IF;
        RAISE;
    END;
    INSERT INTO harness_shape VALUES (tv::regclass, key);
    PERFORM assert_fresh(tv::regclass, key, 'creation');
END $f$;

-- plain
SELECT harness_create('tv_customer', 'pk_customer', $$
  SELECT c.pk_customer, c.id, jsonb_build_object('name', c.name) AS data FROM tb_customer c $$);
-- root table not named tb_<entity> (#175)
SELECT harness_create('tv_purchase', 'pk_purchase', $$
  SELECT o.pk_order AS pk_purchase, o.id, jsonb_build_object('ref', o.ref, 'c', c.name) AS data
  FROM tb_order o JOIN tb_customer c ON c.pk_customer = o.fk_customer $$);
-- mapped join (two hops to tb_customer)
SELECT harness_create('tv_line', 'pk_line', $$
  SELECT l.pk_line, l.id, l.fk_order,
         jsonb_build_object('sku', l.sku, 'qty', l.qty, 'customer', c.name) AS data
  FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order
  JOIN tb_customer c ON c.pk_customer = o.fk_customer $$);
-- DISTINCT ON a unique root column, joined tables, embedding a plain TVIEW (#169)
SELECT harness_create('tv_order', 'id', $$
  SELECT DISTINCT ON (o.id) o.pk_order, o.id, o.fk_customer,
         jsonb_build_object('ref', o.ref, 'status', o.status, 'n', v.n, 'qty', v.qty,
                            'customer', vc.data) AS data
  FROM tb_order o LEFT JOIN v_cnt v ON v.fk_order = o.pk_order
  LEFT JOIN v_customer vc ON vc.pk_customer = o.fk_customer ORDER BY o.id $$);
-- DISTINCT ON the root key, aliased as pk_<entity> (versioned rows). The writes can
-- tie two versions: the ORDER BY ends with the pk so the winner is deterministic.
SELECT harness_create('tv_contract', 'pk_contract', $$
  SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, c.id,
         jsonb_build_object('status', c.status, 'version', c.version_no) AS data
  FROM tb_contract c ORDER BY c.id_contract, c.version_no DESC, c.pk_contract DESC $$);
-- parents of a DISTINCT ON TVIEW, one and two levels up
SELECT harness_create('tv_deal', 'pk_deal', $$
  SELECT d.pk_deal, d.id, d.fk_contract,
         jsonb_build_object('name', d.name, 'contract', vc.data) AS data
  FROM tb_deal d LEFT JOIN v_contract vc ON vc.pk_contract = d.fk_contract $$);
SELECT harness_create('tv_desk', 'pk_desk', $$
  SELECT k.pk_desk, k.id, k.fk_deal, jsonb_build_object('label', k.label, 'deal', vd.data) AS data
  FROM tb_desk k LEFT JOIN v_deal vd ON vd.pk_deal = k.fk_deal $$);
-- DISTINCT ON a non-unique uuid: the winning row's pk_<entity> changes
SELECT harness_create('tv_doc', 'id', $$
  SELECT DISTINCT ON (d.id) d.pk_doc, d.id, jsonb_build_object('body', d.body, 'rev', d.rev) AS data
  FROM tb_doc d ORDER BY d.id, d.rev DESC, d.pk_doc DESC $$);
SELECT harness_create('tv_note', 'pk_note', $$
  SELECT n.pk_note, n.id, n.fk_doc, jsonb_build_object('text', n.text, 'doc', vd.data) AS data
  FROM tb_note n LEFT JOIN v_doc vd ON vd.pk_doc = n.fk_doc $$);
-- DISTINCT ON a joined table's column: the last line of each order
SELECT harness_create('tv_lastline', 'pk_lastline', $$
  SELECT DISTINCT ON (l.fk_order) l.fk_order AS pk_lastline, o.id,
         jsonb_build_object('sku', l.sku, 'ref', o.ref) AS data
  FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order ORDER BY l.fk_order, l.pk_line DESC $$);
-- generated columns (virtual on PostgreSQL 18): read, and joined on (#179)
SELECT harness_create('tv_vnote', 'pk_vnote', $$
  SELECT n.pk_vnote, n.id, n.fk_customer,
         jsonb_build_object('tag', n.tag, 'customer', c.name_up,
                            'tagged', (SELECT count(*) FROM tb_vtag t WHERE t.norm = n.tag)) AS data
  FROM tb_vnote n LEFT JOIN tb_customer c ON c.pk_customer = n.fk_customer $$);
-- UNION
SELECT harness_create('tv_task', 'pk_task', $$
  SELECT pk_task, id, jsonb_build_object('title', title, 'archived', false) AS data FROM tb_task
  UNION ALL
  SELECT pk_task, id, jsonb_build_object('title', title, 'archived', true) AS data FROM tb_task_archive $$);
-- aggregate
SELECT harness_create('tv_custstat', 'pk_custstat', $$
  SELECT o.fk_customer AS pk_custstat, c.id,
         jsonb_build_object('name', c.name, 'orders', count(*)) AS data
  FROM tb_order o JOIN tb_customer c ON c.pk_customer = o.fk_customer
  GROUP BY o.fk_customer, c.id, c.name $$, '{"tb_order": "fk_customer", "tb_customer": "pk_customer"}');
-- the ancestors of a node through its path of ids, three spellings (#182)
SELECT harness_create('tv_nsrf', 'pk_nsrf', $$
  SELECT s.pk_node AS pk_nsrf, s.id,
         jsonb_build_object('name', s.name, 'up', array_agg(a.name ORDER BY a.pk_node)) AS data
  FROM (SELECT n.pk_node, n.id, n.name, unnest(string_to_array(n.path, '.')::int[]) AS node_id
        FROM tb_node n WHERE n.deleted_at IS NULL) s
  JOIN tb_node a ON a.pk_node = s.node_id AND a.deleted_at IS NULL
  GROUP BY s.pk_node, s.id, s.name $$);
SELECT harness_create('tv_nlat', 'pk_nlat', $$
  SELECT n.pk_node AS pk_nlat, n.id,
         jsonb_build_object('name', n.name, 'up', array_agg(a.name ORDER BY a.pk_node)) AS data
  FROM tb_node n CROSS JOIN LATERAL unnest(string_to_array(n.path, '.')::int[]) AS u(node_id)
  JOIN tb_node a ON a.pk_node = u.node_id AND a.deleted_at IS NULL
  WHERE n.deleted_at IS NULL GROUP BY n.pk_node, n.id, n.name $$);
SELECT harness_create('tv_nany', 'pk_nany', $$
  SELECT n.pk_node AS pk_nany, n.id,
         jsonb_build_object('name', n.name, 'up', array_agg(a.name ORDER BY a.pk_node)) AS data
  FROM tb_node n JOIN tb_node a ON a.pk_node = ANY (string_to_array(n.path, '.')::int[])
                                AND a.deleted_at IS NULL
  WHERE n.deleted_at IS NULL GROUP BY n.pk_node, n.id, n.name $$);
-- a join on a computed output of a subquery (#182)
SELECT harness_create('tv_nbadge', 'pk_nbadge', $$
  SELECT s.pk_node AS pk_nbadge, s.id,
         jsonb_build_object('name', s.name, 'badges', count(x.pk_badge)) AS data
  FROM (SELECT n.pk_node, n.id, n.name, upper(n.name) AS upper_name FROM tb_node n) s
  LEFT JOIN tb_badge x ON x.code = s.upper_name GROUP BY s.pk_node, s.id, s.name $$);
-- a recursive view read by the item view; the tree is refreshed in full (#183)
SELECT harness_create('tv_item', 'pk_item', $$
  SELECT i.pk_item, i.id, jsonb_build_object('name', i.name, 'path', cp.names) AS data
  FROM tb_item i LEFT JOIN v_category_path cp ON cp.pk_category = i.fk_category $$,
  policy => 'full_refresh');

-- ── writes ───────────────────────────────────────────────────────────────────
-- `n` rows of `tbl` from a random offset, as a subquery of their pks.
CREATE FUNCTION harness_pick(tbl text, pk text, n int) RETURNS text LANGUAGE sql AS $$
  SELECT format('(SELECT %2$s FROM %1$s ORDER BY %2$s OFFSET %3$s LIMIT %4$s)',
                tbl, pk, floor(random() * 12)::int, n) $$;
CREATE FUNCTION harness_n() RETURNS int LANGUAGE sql AS $$
  SELECT CASE WHEN random() < 0.5 THEN 1 ELSE 2 + floor(random() * 3)::int END $$;
CREATE FUNCTION harness_i(hi int) RETURNS int LANGUAGE sql AS $$
  SELECT 1 + floor(random() * hi)::int $$;

-- One random statement.
CREATE FUNCTION harness_statement(i int) RETURNS text LANGUAGE plpgsql AS $$
DECLARE r int := floor(random() * 42)::int;
BEGIN
    RETURN CASE r
    WHEN 0 THEN format('INSERT INTO tb_customer (name) SELECT ''c%s_'' || g FROM generate_series(1, %s) g', i, harness_n())
    WHEN 1 THEN format('UPDATE tb_customer SET name = name || ''.%s'' WHERE pk_customer IN %s', i, harness_pick('tb_customer', 'pk_customer', harness_n()))
    WHEN 2 THEN format('DELETE FROM tb_customer WHERE pk_customer IN %s', harness_pick('tb_customer', 'pk_customer', 1))
    WHEN 3 THEN format('INSERT INTO tb_order (fk_customer, ref, status) SELECT %s, ''r%s_'' || g, ''new'' FROM generate_series(1, %s) g', harness_i(8), i, harness_n())
    WHEN 4 THEN format('UPDATE tb_order SET status = ''s%s'' WHERE pk_order IN %s', i, harness_pick('tb_order', 'pk_order', harness_n()))
    WHEN 5 THEN format('UPDATE tb_order SET fk_customer = %s WHERE pk_order IN %s', harness_i(8), harness_pick('tb_order', 'pk_order', harness_n()))
    WHEN 6 THEN format('DELETE FROM tb_order WHERE pk_order IN %s', harness_pick('tb_order', 'pk_order', 1))
    WHEN 7 THEN format('INSERT INTO tb_line (fk_order, sku, qty) SELECT %s, ''l%s_'' || g, g FROM generate_series(1, %s) g', harness_i(14), i, harness_n())
    WHEN 8 THEN format('UPDATE tb_line SET qty = qty + 1, sku = sku || ''.%s'' WHERE pk_line IN %s', i, harness_pick('tb_line', 'pk_line', harness_n()))
    WHEN 9 THEN format('UPDATE tb_line SET fk_order = %s WHERE pk_line IN %s', harness_i(14), harness_pick('tb_line', 'pk_line', harness_n()))
    WHEN 10 THEN format('DELETE FROM tb_line WHERE pk_line IN %s', harness_pick('tb_line', 'pk_line', harness_n()))
    WHEN 11 THEN format('INSERT INTO tb_contract (id_contract, version_no, status) SELECT 100 * %s, coalesce(max(version_no), 0) + 1, ''v%s'' FROM tb_contract WHERE id_contract = 100 * %s', harness_i(7), i, harness_i(7))
    WHEN 12 THEN format('UPDATE tb_contract SET status = ''u%s'' WHERE id_contract IN (SELECT DISTINCT id_contract FROM tb_contract ORDER BY 1 OFFSET %s LIMIT %s)', i, floor(random() * 4)::int, harness_n())
    WHEN 13 THEN format('UPDATE tb_contract SET id_contract = 100 * %s WHERE pk_contract IN %s', harness_i(7), harness_pick('tb_contract', 'pk_contract', harness_n()))
    WHEN 14 THEN format('DELETE FROM tb_contract WHERE pk_contract IN %s', harness_pick('tb_contract', 'pk_contract', harness_n()))
    WHEN 15 THEN format('INSERT INTO tb_deal (fk_contract, name) VALUES (100 * %s, ''d%s'')', harness_i(7), i)
    WHEN 16 THEN format('UPDATE tb_deal SET fk_contract = 100 * %s, name = name || ''.%s'' WHERE pk_deal IN %s', harness_i(7), i, harness_pick('tb_deal', 'pk_deal', harness_n()))
    WHEN 17 THEN format('INSERT INTO tb_desk (fk_deal, label) VALUES (%s, ''k%s'')', harness_i(8), i)
    WHEN 18 THEN format('UPDATE tb_desk SET fk_deal = %s WHERE pk_desk IN %s', harness_i(8), harness_pick('tb_desk', 'pk_desk', 1))
    WHEN 19 THEN format('INSERT INTO tb_doc (id, rev, body) SELECT ''00000000-0000-0000-0000-00000000000%s'', coalesce(max(rev), 0) + 1, ''b%s'' FROM tb_doc WHERE id = ''00000000-0000-0000-0000-00000000000%s''', harness_i(5), i, harness_i(5))
    WHEN 20 THEN format('UPDATE tb_doc SET body = body || ''.%s'' WHERE pk_doc IN %s', i, harness_pick('tb_doc', 'pk_doc', harness_n()))
    WHEN 21 THEN format('UPDATE tb_doc SET id = ''00000000-0000-0000-0000-00000000000%s'' WHERE pk_doc IN %s', harness_i(5), harness_pick('tb_doc', 'pk_doc', harness_n()))
    WHEN 22 THEN format('DELETE FROM tb_doc WHERE pk_doc IN %s', harness_pick('tb_doc', 'pk_doc', 1))
    WHEN 23 THEN format('UPDATE tb_note SET fk_doc = %s WHERE pk_note IN %s', harness_i(10), harness_pick('tb_note', 'pk_note', harness_n()))
    WHEN 24 THEN format('UPDATE tb_task SET title = title || ''.%s'' WHERE pk_task IN %s', i, harness_pick('tb_task', 'pk_task', harness_n()))
    WHEN 25 THEN format('INSERT INTO tb_task_archive (title) VALUES (''a%s'')', i)
    WHEN 26 THEN format('UPDATE tb_vtag SET raw = chr(65 + %s) WHERE pk_vtag IN %s', floor(random() * 6)::int, harness_pick('tb_vtag', 'pk_vtag', harness_n()))
    WHEN 27 THEN format('INSERT INTO tb_vtag (raw) VALUES (chr(65 + %s))', floor(random() * 6)::int)
    WHEN 28 THEN format('DELETE FROM tb_vtag WHERE pk_vtag IN %s', harness_pick('tb_vtag', 'pk_vtag', 1))
    WHEN 29 THEN format('INSERT INTO tb_node (pk_node, path, name) SELECT m.k, p.path || ''.'' || m.k, ''n'' || m.k FROM (SELECT max(pk_node) + 1 AS k FROM tb_node) m, tb_node p WHERE p.pk_node = %s', harness_i(8))
    WHEN 30 THEN format('UPDATE tb_node SET name = name || ''.%s'' WHERE pk_node IN %s', i, harness_pick('tb_node', 'pk_node', harness_n()))
    WHEN 31 THEN format('UPDATE tb_node SET deleted_at = CASE WHEN deleted_at IS NULL THEN now() END WHERE pk_node IN %s', harness_pick('tb_node', 'pk_node', harness_n()))
    WHEN 32 THEN format('UPDATE tb_node SET path = ''1.%s.'' || pk_node WHERE pk_node IN %s', harness_i(8), harness_pick('tb_node', 'pk_node', 1))
    WHEN 33 THEN format('DELETE FROM tb_node WHERE pk_node IN %s', harness_pick('tb_node', 'pk_node', 1))
    WHEN 34 THEN format('UPDATE tb_node SET name = ''n%s'' WHERE pk_node IN %s', harness_i(5), harness_pick('tb_node', 'pk_node', 1))
    WHEN 35 THEN format('INSERT INTO tb_badge (code, label) VALUES (''N%s'', ''b%s'')', harness_i(8), i)
    WHEN 36 THEN format('UPDATE tb_badge SET code = ''N%s'' WHERE pk_badge IN %s', harness_i(8), harness_pick('tb_badge', 'pk_badge', harness_n()))
    WHEN 37 THEN format('DELETE FROM tb_badge WHERE pk_badge IN %s', harness_pick('tb_badge', 'pk_badge', 1))
    WHEN 38 THEN format('UPDATE tb_category SET name = name || ''.%s'' WHERE pk_category IN %s', i, harness_pick('tb_category', 'pk_category', harness_n()))
    WHEN 39 THEN format('INSERT INTO tb_category (pk_category, fk_parent, name) SELECT max(pk_category) + 1, %s, ''k%s'' FROM tb_category', harness_i(6), i)
    WHEN 40 THEN format('UPDATE tb_item SET fk_category = %s, name = name || ''.%s'' WHERE pk_item IN %s', harness_i(7), i, harness_pick('tb_item', 'pk_item', harness_n()))
    ELSE format('UPDATE tb_vnote SET tag = chr(97 + %s), fk_customer = %s WHERE pk_vnote IN %s', floor(random() * 6)::int, harness_i(8), harness_pick('tb_vnote', 'pk_vnote', harness_n()))
    END;
END $$;

-- Raise unless every TVIEW equals its view; a TVIEW listed in the setting
-- harness.xfail only reports (once) that it diverged.
CREATE FUNCTION harness_check(label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    s record;
    diff text;
    xfail text[] := string_to_array(coalesce(current_setting('harness.xfail', true), ''), ',');
    failures text := '';
BEGIN
    FOR s IN SELECT tv, key FROM harness_shape ORDER BY tv::text LOOP
        diff := fresh_diff(s.tv, s.key);
        CONTINUE WHEN diff IS NULL;
        IF s.tv::text = ANY (xfail) THEN
            IF NOT s.tv::text = ANY (string_to_array(coalesce(current_setting('harness.diverged', true), ''), ',')) THEN
                RAISE WARNING 'XFAIL % diverged after %: %', s.tv, label, diff;
                PERFORM set_config('harness.diverged',
                    concat_ws(',', nullif(current_setting('harness.diverged', true), ''), s.tv::text), false);
            END IF;
        ELSE
            failures := failures || E'\n  ' || diff;
        END IF;
    END LOOP;
    IF failures <> '' THEN
        RAISE EXCEPTION 'DIVERGED after %:%', label, failures;
    END IF;
END $$;

-- The statements of one run, each followed by its check.
CREATE FUNCTION harness_script(n int) RETURNS SETOF text LANGUAGE plpgsql AS $$
DECLARE stmt text;
BEGIN
    FOR i IN 1..n LOOP
        stmt := harness_statement(i);
        RETURN NEXT stmt;
        RETURN NEXT format('SELECT harness_check(%L)', format('#%s %s', i, stmt));
    END LOOP;
END $$;
