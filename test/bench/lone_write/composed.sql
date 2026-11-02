-- The embed workload's data (test/concurrency/embed.sql: 200 items, 4 places, 8
-- groups) with the counts composed at read time instead of embedded: tv_place and
-- tv_group each hold the count of their current items, and tv_item holds
-- fk_place and fk_group, which a reader joins to them. A soft-delete rewrites one
-- item, one place and one group, not every item of the place and the group.
SET client_min_messages = warning;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE TABLE tb_place (pk_place bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_group (pk_group bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_place bigint NOT NULL REFERENCES tb_place,
                      fk_group bigint NOT NULL REFERENCES tb_group,
                      name text, start_date date NOT NULL, deleted_at timestamptz);
CREATE INDEX ON tb_item (fk_place);
CREATE INDEX ON tb_item (fk_group);
INSERT INTO tb_place (pk_place, name) SELECT g, 'p' || g FROM generate_series(1, 4) g;
INSERT INTO tb_group (pk_group, name) SELECT g, 'g' || g FROM generate_series(1, 8) g;
INSERT INTO tb_item (pk_item, fk_place, fk_group, name, start_date)
  SELECT g, 1 + g % 4, 1 + (g / 4) % 8, 'i' || g, DATE '2026-01-01'
  FROM generate_series(1, 200) g;
SELECT tviews.pg_tviews_create('tv_place', $$
  SELECT p.pk_place, p.id,
         jsonb_build_object('name', p.name,
           'current', (SELECT count(*) FROM tb_item i
                       WHERE i.fk_place = p.pk_place AND i.deleted_at IS NULL
                         AND i.start_date <= DATE '2026-06-01')) AS data
  FROM tb_place p $$);
SELECT tviews.pg_tviews_create('tv_group', $$
  SELECT g.pk_group, g.id,
         jsonb_build_object('name', g.name,
           'current', (SELECT count(*) FROM tb_item i
                       WHERE i.fk_group = g.pk_group AND i.deleted_at IS NULL
                         AND i.start_date <= DATE '2026-06-01')) AS data
  FROM tb_group g $$);
SELECT tviews.pg_tviews_create('tv_item', $$
  SELECT i.pk_item, i.id, i.fk_place, i.fk_group,
         jsonb_build_object('name', i.name, 'deleted', i.deleted_at IS NOT NULL) AS data
  FROM tb_item i $$);
