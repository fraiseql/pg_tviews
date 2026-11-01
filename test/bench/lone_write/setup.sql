-- The embed workload's schema (test/concurrency/embed.sql: 200 items, 4 places,
-- 8 groups, each item showing the counts of current items in its place and its
-- group), plus tv_group (its items' names), tv_card (an item with its group's and
-- place's names), and tv_owner, which tv_lot, tv_tag and tv_ledger embed.
\set items 200
\set places 4
\set groups 8
\ir ../../concurrency/embed.sql
SELECT tviews.pg_tviews_create('tv_group', $$
  SELECT g.pk_group, g.id,
         jsonb_build_object('name', g.name,
           'items', (SELECT jsonb_agg(i.name ORDER BY i.pk_item) FROM tb_item i
                     WHERE i.fk_group = g.pk_group AND i.deleted_at IS NULL)) AS data
  FROM tb_group g $$);
SELECT tviews.pg_tviews_create('tv_card', $$
  SELECT i.pk_item AS pk_card, i.id,
         jsonb_build_object('name', i.name, 'group', g.name, 'place', p.name) AS data
  FROM tb_item i JOIN tb_group g ON g.pk_group = i.fk_group
  JOIN tb_place p ON p.pk_place = i.fk_place $$);
CREATE TABLE tb_owner (pk_owner bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_owner (pk_owner, name) SELECT g, 'o' || g FROM generate_series(1, 8) g;
SELECT tviews.pg_tviews_create('tv_owner', $$
  SELECT o.pk_owner, o.id, jsonb_build_object('name', o.name) AS data FROM tb_owner o $$);
SELECT tviews.pg_tviews_create('tv_lot', $$
  SELECT i.pk_item AS pk_lot, i.id, jsonb_build_object('name', i.name, 'owner', o.data) AS data
  FROM tb_item i JOIN tv_owner o ON o.pk_owner = i.fk_group $$);
SELECT tviews.pg_tviews_create('tv_tag', $$
  SELECT i.pk_item AS pk_tag, i.id, jsonb_build_object('owner', o.data, 'place', i.fk_place) AS data
  FROM tb_item i JOIN tv_owner o ON o.pk_owner = i.fk_group $$);
SELECT tviews.pg_tviews_create('tv_ledger', $$
  SELECT g.pk_group AS pk_ledger, g.id, jsonb_build_object('group', g.name, 'owner', o.data) AS data
  FROM tb_group g JOIN tv_owner o ON o.pk_owner = g.pk_group $$);
