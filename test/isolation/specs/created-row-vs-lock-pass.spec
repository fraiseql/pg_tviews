# What a write's capture can still deadlock on (#277): a row created and
# committed between its two lookups. Every item shows its place's count of
# current items and its group's name. The creator inserts item 5 into place 1,
# group 5, and stays open: it holds place 1 and, having changed a link, what item
# 5 reads, group 5 among it. The renamer then renames group 5 (waiting for the
# creator), and the writer soft-deletes item 1 of place 1 (waiting for the
# creator too); its first lookup found items 1, 3 and 4. Once the creator commits,
# the renamer holds group 5 and the writer's pass completes. The writer's second
# lookup finds item 5 as well, whose group the renamer holds: it waits for the
# renamer. The renamer's next write, item 3 of place 1, waits for the writer:
# PostgreSQL fails the writer with 40P01. Without the creation (second
# permutation) the second lookup finds nothing new, and the renamer waits for the
# writer to commit. The TVIEW is fresh either way.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_place (pk_place bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text);
  CREATE TABLE tb_group (pk_group bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text);
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_place bigint NOT NULL REFERENCES tb_place,
                        fk_group bigint NOT NULL REFERENCES tb_group, name text,
                        deleted_at timestamptz);
  INSERT INTO tb_place (pk_place, name) VALUES (1, 'p1');
  INSERT INTO tb_group (pk_group, name) VALUES (1, 'g1'), (5, 'g5');
  INSERT INTO tb_item (pk_item, fk_place, fk_group, name)
    VALUES (1, 1, 1, 'i1'), (3, 1, 1, 'i3'), (4, 1, 1, 'i4');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id,
           jsonb_build_object('name', i.name, 'place', pc.n, 'group', g.name) AS data
    FROM tb_item i
    JOIN tb_group g ON g.pk_group = i.fk_group
    JOIN (SELECT fk_place, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
          GROUP BY fk_place) pc ON pc.fk_place = i.fk_place $$);
}

teardown
{
  DROP TABLE tv_item;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_item, tb_group, tb_place;
}

session creator
step c_begin   { BEGIN; }
step c_insert  { INSERT INTO tb_item (pk_item, fk_place, fk_group, name) VALUES (5, 1, 5, 'i5'); }
step c_commit  { COMMIT; }

session renamer
setup          { SET deadlock_timeout = '10s'; }
step r_begin   { BEGIN; }
step r_rename  { UPDATE tb_group SET name = 'g5!' WHERE pk_group = 5; }
step r_delete  { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 3; }
step r_commit  { COMMIT; }

session writer
setup          { SET deadlock_timeout = '100ms'; }
step w_delete  { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 1; }
step w_check
{
  SELECT t.pk_item, t.data = v.data AS fresh
  FROM tv_item t FULL JOIN tviews.public__tv_item v USING (pk_item) ORDER BY 1;
}

permutation c_begin c_insert r_begin r_rename w_delete c_commit r_delete r_commit w_check
permutation r_begin r_rename w_delete r_delete r_commit w_check
