-- #179: a write that reaches a TVIEW only through a virtual generated column
-- (PostgreSQL 18's default kind) was lost. Such a column is NULL in the rows a
-- trigger sees and in transition tables; only the view computes it. A TVIEW reading
-- one now reads its inputs, the changed rows used for key mapping compute it, and
-- a key on one is mapped instead of read off the row.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_virtual_generated_columns.sql
--
-- expect-output: #179 virtual generated columns: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
SELECT current_setting('server_version_num')::int >= 180000 AS pg18 \gset
\if :pg18
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_region (pk_region int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  label text, tag text GENERATED ALWAYS AS (label || '!'));
CREATE TABLE tb_shop (pk_shop int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  name text, code text GENERATED ALWAYS AS (upper(name)), fk_region int);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  price numeric, taxed numeric GENERATED ALWAYS AS (price * 1.2), fk_shop int);
INSERT INTO tb_region VALUES (1, default, 'r1'), (2, default, 'r2');
INSERT INTO tb_shop VALUES (1, default, 's1', default, 1), (2, default, 's2', default, 2);
INSERT INTO tb_item VALUES (1, default, 10, default, 1), (2, default, 20, default, 2);

-- ── V1: a joined local table read through a virtual column ──────────────────
SELECT pg_tviews_create('tv_item', $$
  SELECT i.pk_item, i.id, i.fk_shop, jsonb_build_object('taxed', i.taxed, 'shop', s.code) AS data
  FROM tb_item i JOIN tb_shop s ON s.pk_shop = i.fk_shop $$);
UPDATE tb_shop SET name = 'zz' WHERE pk_shop = 1;
SELECT assert_fresh('tv_item', 'pk_item', 'V1: an input of a joined virtual column');
UPDATE tb_item SET price = 11 WHERE pk_item = 2;
SELECT assert_fresh('tv_item', 'pk_item', 'an input of the root''s virtual column');

-- ── V2: a mapped table read through a virtual column ────────────────────────
SELECT pg_tviews_create('tv_line', $$
  SELECT i.pk_item AS pk_line, i.id, jsonb_build_object('region', r.tag) AS data
  FROM tb_item i JOIN tb_shop s ON s.pk_shop = i.fk_shop JOIN tb_region r ON r.pk_region = s.fk_region $$);
UPDATE tb_region SET label = 'r9' WHERE pk_region = 1;
SELECT assert_fresh('tv_line', 'pk_line', 'V2: an input of a mapped virtual column');
UPDATE tb_region SET label = label || '+';
SELECT assert_fresh('tv_line', 'pk_line', 'V2: a two-row UPDATE');

-- ── V3: a join on a virtual column ──────────────────────────────────────────
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  raw text, norm text GENERATED ALWAYS AS (lower(raw)));
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), tag text);
INSERT INTO tb_tag VALUES (1, default, 'A');
INSERT INTO tb_note VALUES (1, default, 'a'), (2, default, 'b');
SELECT pg_tviews_create('tv_note', $$
  SELECT n.pk_note, n.id, jsonb_build_object('tag', n.tag, 'tagged', t.pk_tag) AS data
  FROM tb_note n LEFT JOIN tb_tag t ON t.norm = n.tag $$);
UPDATE tb_tag SET raw = 'B';
SELECT assert_fresh('tv_note', 'pk_note', 'V3: a join key moving');
INSERT INTO tb_tag VALUES (2, default, 'A');
SELECT assert_fresh('tv_note', 'pk_note', 'V3: an INSERT');
DELETE FROM tb_tag WHERE pk_tag = 1;
SELECT assert_fresh('tv_note', 'pk_note', 'V3: a DELETE');

-- ── a local key on a virtual column ─────────────────────────────────────────
CREATE TABLE tb_ref (pk_ref int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  raw text, ord int GENERATED ALWAYS AS (raw::int), note text);
INSERT INTO tb_ref VALUES (1, default, '1', default, 'n1');
SELECT pg_tviews_create('tv_order', $$
  SELECT i.pk_item AS pk_order, i.id, jsonb_build_object('note', r.note) AS data
  FROM tb_item i LEFT JOIN tb_ref r ON r.ord = i.pk_item $$);
UPDATE tb_ref SET raw = '2';
SELECT assert_fresh('tv_order', 'pk_order', 'a local key on a virtual column moving');
UPDATE tb_ref SET note = 'n2';
SELECT assert_fresh('tv_order', 'pk_order', 'a row with a virtual local key');

-- ── a DISTINCT ON key on a virtual column ───────────────────────────────────
CREATE TABLE tb_ver (pk_ver int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  raw text, code text GENERATED ALWAYS AS (upper(raw)), rev int, body text);
INSERT INTO tb_ver VALUES (1, default, 'a', default, 1, 'b1'), (2, default, 'a', default, 2, 'b2'),
                          (3, default, 'b', default, 1, 'b3');
SELECT pg_tviews_create('tv_ver', $$
  SELECT DISTINCT ON (v.code) v.pk_ver, v.id, v.code, jsonb_build_object('body', v.body) AS data
  FROM tb_ver v ORDER BY v.code, v.rev DESC $$);
UPDATE tb_ver SET body = body || '!';
SELECT assert_fresh('tv_ver', 'code', 'a DISTINCT ON TVIEW keyed on a virtual column');
UPDATE tb_ver SET raw = 'c' WHERE pk_ver = 2;
SELECT assert_fresh('tv_ver', 'code', 'a row moving to another virtual key');

-- ── direct patch: data with an input and the virtual column computed from it ─
CREATE TABLE tb_prod (pk_prod int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  price numeric, taxed numeric GENERATED ALWAYS AS (price * 1.2));
INSERT INTO tb_prod VALUES (1, default, 10);
SELECT pg_tviews_create('tv_prod', $$
  SELECT pk_prod, id, jsonb_build_object('price', price, 'taxed', taxed) AS data FROM tb_prod $$);
UPDATE tb_prod SET price = 15;
SELECT assert_fresh('tv_prod', 'pk_prod', 'an input copied into data next to its virtual column');

-- ── fan-out: a parent field and a virtual column computed from it ───────────
CREATE TABLE tb_cat (pk_cat int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  name text, name_up text GENERATED ALWAYS AS (upper(name)));
CREATE TABLE tb_good (pk_good int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  fk_cat int, label text);
CREATE INDEX ON tb_good (fk_cat);
INSERT INTO tb_cat VALUES (1, default, 'c1');
INSERT INTO tb_good VALUES (1, default, 1, 'g1'), (2, default, 1, 'g2');
SELECT pg_tviews_create('tv_good', $$
  SELECT g.pk_good, g.id, g.fk_cat,
         jsonb_build_object('label', g.label, 'cat', c.name, 'cat_up', c.name_up) AS data
  FROM tb_good g JOIN tb_cat c ON c.pk_cat = g.fk_cat $$);
UPDATE tb_cat SET name = 'c2';
SELECT assert_fresh('tv_good', 'pk_good', 'a parent field copied next to its virtual column');

-- ── a partitioned mapped table with a virtual column ────────────────────────
CREATE TABLE tb_zone (pk_zone int, id uuid NOT NULL DEFAULT gen_random_uuid(), label text,
  tag text GENERATED ALWAYS AS (label || '#'), PRIMARY KEY (pk_zone)) PARTITION BY RANGE (pk_zone);
CREATE TABLE tb_zone_a PARTITION OF tb_zone FOR VALUES FROM (0) TO (100);
ALTER TABLE tb_region ADD COLUMN fk_zone int;
INSERT INTO tb_zone VALUES (1, default, 'z1');
UPDATE tb_region SET fk_zone = 1;
SELECT pg_tviews_create('tv_place', $$
  SELECT i.pk_item AS pk_place, i.id, jsonb_build_object('zone', z.tag) AS data
  FROM tb_item i JOIN tb_shop s ON s.pk_shop = i.fk_shop
  JOIN tb_region r ON r.pk_region = s.fk_region JOIN tb_zone z ON z.pk_zone = r.fk_zone $$);
UPDATE tb_zone SET label = 'z2';
SELECT assert_fresh('tv_place', 'pk_place', 'a partitioned table''s virtual column');

-- ── control: a STORED generated column ──────────────────────────────────────
CREATE TABLE tb_kiosk (pk_kiosk int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  name text, code text GENERATED ALWAYS AS (upper(name)) STORED);
CREATE TABLE tb_stand (pk_stand int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_kiosk int);
INSERT INTO tb_kiosk VALUES (1, default, 'k'); INSERT INTO tb_stand VALUES (1, default, 1);
SELECT pg_tviews_create('tv_stand', $$
  SELECT st.pk_stand, st.id, jsonb_build_object('kiosk', k.code) AS data
  FROM tb_stand st JOIN tb_kiosk k ON k.pk_kiosk = st.fk_kiosk $$);
UPDATE tb_kiosk SET name = 'k2';
SELECT assert_fresh('tv_stand', 'pk_stand', 'a STORED generated column');

\echo '#179 virtual generated columns: PASS'
\else
\echo '#179 virtual generated columns: PASS (skipped: no virtual generated columns before PostgreSQL 18)'
\endif
