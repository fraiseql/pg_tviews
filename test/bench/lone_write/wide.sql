-- One TVIEW whose read set joins nine tables: an item with its asset (-> model ->
-- vendor), config, agreement, place and group, and the counts of current items in
-- its place and its group. 2,000 items.
SET client_min_messages = warning;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE TABLE tb_vendor (pk_vendor bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_model (pk_model bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_vendor bigint NOT NULL REFERENCES tb_vendor, name text);
CREATE TABLE tb_asset (pk_asset bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_model bigint NOT NULL REFERENCES tb_model, serial text);
CREATE TABLE tb_config (pk_config bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), label text);
CREATE TABLE tb_agreement (pk_agreement bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_place (pk_place bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_group (pk_group bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
  fk_place bigint NOT NULL REFERENCES tb_place, fk_group bigint NOT NULL REFERENCES tb_group,
  fk_asset bigint NOT NULL REFERENCES tb_asset, fk_config bigint NOT NULL REFERENCES tb_config,
  fk_agreement bigint NOT NULL REFERENCES tb_agreement,
  name text, start_date date NOT NULL, deleted_at timestamptz);
CREATE INDEX ON tb_item (fk_place); CREATE INDEX ON tb_item (fk_group); CREATE INDEX ON tb_item (fk_asset);
CREATE INDEX ON tb_item (fk_config); CREATE INDEX ON tb_item (fk_agreement);
CREATE INDEX ON tb_model (fk_vendor); CREATE INDEX ON tb_asset (fk_model);
INSERT INTO tb_vendor (pk_vendor, name) SELECT g, 'v' || g FROM generate_series(1, 20) g;
INSERT INTO tb_model (pk_model, fk_vendor, name) SELECT g, 1 + g % 20, 'm' || g FROM generate_series(1, 100) g;
INSERT INTO tb_asset (pk_asset, fk_model, serial) SELECT g, 1 + g % 100, 's' || g FROM generate_series(1, 2000) g;
INSERT INTO tb_config (pk_config, label) SELECT g, 'c' || g FROM generate_series(1, 200) g;
INSERT INTO tb_agreement (pk_agreement, name) SELECT g, 'a' || g FROM generate_series(1, 100) g;
INSERT INTO tb_place (pk_place, name) SELECT g, 'p' || g FROM generate_series(1, 40) g;
INSERT INTO tb_group (pk_group, name) SELECT g, 'g' || g FROM generate_series(1, 80) g;
INSERT INTO tb_item (pk_item, fk_place, fk_group, fk_asset, fk_config, fk_agreement, name, start_date)
  SELECT g, 1 + g % 40, 1 + (g / 40) % 80, 1 + g % 2000, 1 + g % 200, 1 + g % 100, 'i' || g, DATE '2026-01-01'
  FROM generate_series(1, 2000) g;
ANALYZE;
SELECT tviews.pg_tviews_create('tv_item', $$
  SELECT i.pk_item, i.id,
         jsonb_build_object('name', i.name,
           'asset', jsonb_build_object('serial', a.serial, 'model', m.name, 'vendor', v.name),
           'config', c.label, 'agreement', ag.name,
           'place', jsonb_build_object('name', p.name, 'current', COALESCE(pc.n, 0)),
           'group', jsonb_build_object('name', g.name, 'current', COALESCE(gc.n, 0))) AS data
  FROM tb_item i
  JOIN tb_asset a ON a.pk_asset = i.fk_asset
  JOIN tb_model m ON m.pk_model = a.fk_model
  JOIN tb_vendor v ON v.pk_vendor = m.fk_vendor
  JOIN tb_config c ON c.pk_config = i.fk_config
  JOIN tb_agreement ag ON ag.pk_agreement = i.fk_agreement
  JOIN tb_place p ON p.pk_place = i.fk_place
  JOIN tb_group g ON g.pk_group = i.fk_group
  LEFT JOIN (SELECT fk_place, count(*) AS n FROM tb_item
             WHERE deleted_at IS NULL AND start_date <= DATE '2026-06-01'
             GROUP BY fk_place) pc ON pc.fk_place = i.fk_place
  LEFT JOIN (SELECT fk_group, count(*) AS n FROM tb_item
             WHERE deleted_at IS NULL AND start_date <= DATE '2026-06-01'
             GROUP BY fk_group) gc ON gc.fk_group = i.fk_group $$);
