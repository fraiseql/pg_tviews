-- The embed workload (test/concurrency/embed.sql) with tb_item split into 200
-- hash partitions, 2,000 items in 40 places and 80 groups: the read sets and
-- the lookups run through a partitioned table. A generic plan locks every
-- partition at each execution; a plan made for its arguments only those it
-- keeps.
SET client_min_messages = warning;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE TABLE tb_place (pk_place bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_group (pk_group bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_item (pk_item bigint NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_place bigint NOT NULL REFERENCES tb_place,
                      fk_group bigint NOT NULL REFERENCES tb_group,
                      name text, start_date date NOT NULL, deleted_at timestamptz,
                      PRIMARY KEY (pk_item)) PARTITION BY HASH (pk_item);
SELECT format('CREATE TABLE tb_item_%s PARTITION OF tb_item FOR VALUES WITH (MODULUS 200, REMAINDER %s)', r, r)
FROM generate_series(0, 199) r \gexec
CREATE INDEX ON tb_item (fk_place);
CREATE INDEX ON tb_item (fk_group);
INSERT INTO tb_place (pk_place, name) SELECT g, 'p' || g FROM generate_series(1, 40) g;
INSERT INTO tb_group (pk_group, name) SELECT g, 'g' || g FROM generate_series(1, 80) g;
INSERT INTO tb_item (pk_item, fk_place, fk_group, name, start_date)
  SELECT g, 1 + g % 40, 1 + (g / 40) % 80, 'i' || g, DATE '2026-01-01'
  FROM generate_series(1, 2000) g;
ANALYZE;
SELECT tviews.pg_tviews_create('tv_item', $$
  SELECT i.pk_item, i.id,
         jsonb_build_object('name', i.name,
           'place', jsonb_build_object('name', p.name, 'current', COALESCE(pc.n, 0)),
           'group', jsonb_build_object('name', g.name, 'current', COALESCE(gc.n, 0))) AS data
  FROM tb_item i
  JOIN tb_place p ON p.pk_place = i.fk_place
  JOIN tb_group g ON g.pk_group = i.fk_group
  LEFT JOIN (SELECT fk_place, count(*) AS n FROM tb_item
             WHERE deleted_at IS NULL AND start_date <= DATE '2026-06-01'
             GROUP BY fk_place) pc ON pc.fk_place = i.fk_place
  LEFT JOIN (SELECT fk_group, count(*) AS n FROM tb_item
             WHERE deleted_at IS NULL AND start_date <= DATE '2026-06-01'
             GROUP BY fk_group) gc ON gc.fk_group = i.fk_group $$);
