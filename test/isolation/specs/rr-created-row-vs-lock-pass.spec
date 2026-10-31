# REPEATABLE READ twin of created-row-vs-lock-pass (#277): a write whose rows a
# concurrent creation links new rows to never waits for the creation and then
# locks what it brought in. A creation still open makes the writer's lock pass
# fail at once with 40001; one committed after the writer's snapshot makes its
# lookup, checked against the latest snapshot, fail with 40001. Retried, the
# write succeeds, and the TVIEW is fresh.

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

session writer
step w_begin   { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT 1 FROM tb_item LIMIT 0; }
step w_delete  { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 1; }
step w_rollback { ROLLBACK; }
step w_retry   { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 1; }
step w_check
{
  SELECT t.pk_item, t.data = v.data AS fresh
  FROM tv_item t FULL JOIN tviews.public__tv_item v USING (pk_item) ORDER BY 1;
}

permutation c_begin c_insert w_begin w_delete w_rollback c_commit w_retry w_check
permutation w_begin c_begin c_insert c_commit w_delete w_rollback w_retry w_check
