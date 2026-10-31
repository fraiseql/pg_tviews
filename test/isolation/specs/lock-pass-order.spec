# The order of a capture's lock pass (#277), when a third transaction makes it
# wait part-way. Every item shows its place's and its group's counts of current
# items and its group's name. The writers soft-delete items 1 (place 1) and 2
# (place 5) of group 1, so each locks its own place and group exclusively and
# reads the other's place. Lock tags are ordered by hash within a relation; here
# place 5 comes before place 1, and both before group 1. tb_group is created after
# tb_item, so its locks come after tb_item's.
#
# - In hash order: the holder renames item 3 (place 1, group 1). The first writer
#   takes place 5 shared, then waits for place 1; the second waits for place 5.
#   Writer locks taken before shared ones would let the second writer hold place
#   5 and group 1 while the first holds place 1: a deadlock.
# - One relation lock for a writer's intent and a refresh's escalation: past a
#   threshold of two values, both writers lock tb_item's values whole. The second
#   writer holds tb_item's intent from an earlier statement (a rename), so the
#   first waits for it. Taken as an intent and then a shared lock, the first
#   writer would hold its intent and the second's shared lock would wait for it.
# - Relation locks first: the holder locks tb_item whole (a threshold of 0), then
#   renames group 1 (a threshold of 64 again, so value by value). The first writer
#   waits for tb_item's intent lock before it takes any value, group 1's among
#   them, so the rename doesn't wait for it.
#
# Every write commits, and the TVIEW is fresh.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_place (pk_place bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text);
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_place bigint NOT NULL REFERENCES tb_place, fk_group bigint NOT NULL,
                        name text, deleted_at timestamptz);
  CREATE TABLE tb_group (pk_group bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text);
  INSERT INTO tb_place (pk_place, name) VALUES (1, 'p1'), (2, 'p2'), (3, 'p3'), (5, 'p5'), (9, 'p9');
  INSERT INTO tb_group (pk_group, name) VALUES (1, 'g1'), (9, 'g9');
  INSERT INTO tb_item (pk_item, fk_place, fk_group, name)
    VALUES (1, 1, 1, 'i1'), (2, 5, 1, 'i2'), (3, 1, 1, 'i3'), (4, 2, 1, 'i4'), (6, 3, 1, 'i6'),
           (9, 9, 9, 'i9');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id,
           jsonb_build_object('name', i.name, 'place', pc.n,
             'group', jsonb_build_object('name', g.name, 'current', gc.n)) AS data
    FROM tb_item i
    JOIN tb_group g ON g.pk_group = i.fk_group
    LEFT JOIN (SELECT fk_place, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
               GROUP BY fk_place) pc ON pc.fk_place = i.fk_place
    LEFT JOIN (SELECT fk_group, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
               GROUP BY fk_group) gc ON gc.fk_group = i.fk_group $$);
}

teardown
{
  DROP TABLE tv_item;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_item, tb_group, tb_place;
}

session holder
setup          { SET deadlock_timeout = '10s'; }
step h_zero    { SET pg_tviews.lock_escalation_threshold = 0; }
step h_values  { SET pg_tviews.lock_escalation_threshold = 64; }
step h_begin   { BEGIN; }
step h_rename3 { UPDATE tb_item SET name = 'i3!' WHERE pk_item = 3; }
step h_delete9 { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 9; }
step h_group   { UPDATE tb_group SET name = 'g1!' WHERE pk_group = 1; }
step h_commit  { COMMIT; }

session one
setup          { SET deadlock_timeout = '100ms'; }
step o_two     { SET pg_tviews.lock_escalation_threshold = 2; }
step o_delete  { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 1; }

session two
setup          { SET deadlock_timeout = '10s'; }
step t_two     { SET pg_tviews.lock_escalation_threshold = 2; }
step t_begin   { BEGIN; }
step t_rename9 { UPDATE tb_item SET name = 'i9!' WHERE pk_item = 9; }
step t_delete  { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 2; }
step t_commit  { COMMIT; }
step t_check
{
  SELECT t.pk_item, t.data = v.data AS fresh
  FROM tv_item t FULL JOIN tviews.public__tv_item v USING (pk_item) ORDER BY 1;
}

permutation h_begin h_rename3 o_delete t_delete h_commit t_check
permutation o_two t_two t_begin t_rename9 o_delete t_delete t_commit t_check
permutation h_zero h_begin h_delete9 o_delete h_values h_group h_commit t_check
