# A prepared transaction keeps its value locks (ADR 0207) until COMMIT PREPARED:
# a crossing write of another session waits for it (isolationtester cannot see a
# wait on a prepared transaction, so the wait ends in a lock timeout), and once it
# commits, the write refreshes from what it committed. Each item shows the count
# of current items in its place; both writers soft-delete an item of place 1.
#
# With the change feed on (publish_changes = rows), PREPARE is refused (a
# notification cannot go through PREPARE, ADR 0231): the transaction rolls back
# and the other write goes through at once.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_place (pk_place bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text);
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_place bigint NOT NULL REFERENCES tb_place, name text,
                        deleted_at timestamptz);
  INSERT INTO tb_place (pk_place, name) VALUES (1, 'p1'), (2, 'p2');
  INSERT INTO tb_item (pk_item, fk_place, name)
    VALUES (1, 1, 'i1'), (2, 1, 'i2'), (3, 1, 'i3'), (4, 2, 'i4');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('name', i.name, 'place', pc.n) AS data
    FROM tb_item i
    LEFT JOIN (SELECT fk_place, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
               GROUP BY fk_place) pc ON pc.fk_place = i.fk_place $$);
}

teardown
{
  DROP TABLE tv_item;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_item, tb_place;
}

session preparer
step p_begin     { BEGIN; }
step p_feed      { SET LOCAL pg_tviews.publish_changes = rows; }
step p_delete    { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 1; }
step p_prepare   { PREPARE TRANSACTION 'pg_tviews_value_locks'; }
step p_held
{
  SELECT count(*) > 0 AS prepared_holds_value_locks FROM pg_locks
  WHERE locktype = 'advisory' AND objsubid = 21622 AND pid IS NULL
    AND classid = 'tb_item'::regclass::oid;
}
step p_commit    { COMMIT PREPARED 'pg_tviews_value_locks'; }

session crosser
setup            { SET lock_timeout = '500ms'; }
step c_delete    { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 2; }
step c_check
{
  SELECT t.pk_item, t.data->'place' AS place, t.data = v.data AS fresh
  FROM tv_item t FULL JOIN tviews.public__tv_item v USING (pk_item) ORDER BY 1;
}

permutation p_begin p_delete p_prepare p_held c_delete p_commit c_delete c_check
permutation p_begin p_feed p_delete p_prepare c_delete c_check
