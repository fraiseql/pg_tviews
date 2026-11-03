# A statement past the escalation threshold locks the relation's values as a
# whole; rolling back to a savepoint taken before it releases that lock in the
# lock manager and in the transaction's record of what it holds (ADR 0207). Each
# item shows the count of current items in its place: a soft-delete changes a
# link, so the writer locks its place's value on tb_item.
#
# The writer soft-deletes items of two places (escalated past a threshold of
# one: tb_item locked whole) and rolls the statement back: it holds nothing. A
# concurrent soft-delete in place 1 goes through at once (no lock leaked). The
# writer then soft-deletes an item of place 2 and holds that value and the
# relation's intent again: taken, not skipped as covered by the rolled-back
# relation lock. Both commit and tv_item is fresh.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_place (pk_place bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text);
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_place bigint NOT NULL REFERENCES tb_place, name text,
                        deleted_at timestamptz);
  INSERT INTO tb_place (pk_place, name) VALUES (1, 'p1'), (2, 'p2'), (3, 'p3');
  INSERT INTO tb_item (pk_item, fk_place, name)
    VALUES (1, 1, 'i1'), (2, 1, 'i2'), (3, 2, 'i3'), (4, 2, 'i4'), (5, 3, 'i5'), (6, 3, 'i6');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('name', i.name, 'place', pc.n) AS data
    FROM tb_item i
    LEFT JOIN (SELECT fk_place, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
               GROUP BY fk_place) pc ON pc.fk_place = i.fk_place $$);
  -- The pg_tviews value and relation locks this backend holds on tb_item.
  CREATE FUNCTION held() RETURNS text[] LANGUAGE sql AS $$
    SELECT coalesce(array_agg(k ORDER BY k), ARRAY[]::text[]) FROM (
      SELECT CASE objsubid WHEN 21622 THEN 'value' ELSE 'relation' END || ':' || mode AS k
      FROM pg_locks
      WHERE locktype = 'advisory' AND objsubid IN (21622, 21623)
        AND classid = 'tb_item'::regclass::oid AND pid = pg_backend_pid()) s $$;
}

teardown
{
  DROP TABLE tv_item;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_item, tb_place;
  DROP FUNCTION held();
}

session writer
setup            { SET pg_tviews.lock_escalation_threshold = 1; }
step w_begin     { BEGIN; SAVEPOINT s; }
step w_two       { UPDATE tb_item SET deleted_at = now() WHERE pk_item IN (1, 3); }
step w_held      { SELECT held(); }
step w_rollback  { ROLLBACK TO SAVEPOINT s; }
step w_four      { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 4; }
step w_commit    { COMMIT; }

session other
setup            { SET lock_timeout = '5s'; }
step o_place1    { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 2; }
step o_check
{
  SELECT t.pk_item, t.data->'place' AS place, t.data = v.data AS fresh
  FROM tv_item t FULL JOIN tviews.public__tv_item v USING (pk_item) ORDER BY 1;
}

permutation w_begin w_two w_held w_rollback w_held o_place1 w_four w_held w_commit o_check
