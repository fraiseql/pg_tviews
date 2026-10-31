# Two TVIEWs read one table, each through its own delta trigger (#277): items
# show their place's count of current items, and zones list their items with
# the same counts. Two transactions each soft-delete an item of one zone, in two
# places. Each delta trigger would lock its own TVIEW's values: the item
# trigger the writer's place, then the zone trigger the zone and the other
# writer's place, held by the other's item trigger. The first delta trigger of
# the statement locks for both TVIEWs in one sorted pass instead. A statement
# trigger between the delta triggers and the flush waits on a gate, one per
# session; the gates open in both orders. Both commit, and the TVIEWs are fresh.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_place (pk_place bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text);
  CREATE TABLE tb_zone (pk_zone bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_place bigint NOT NULL REFERENCES tb_place,
                        fk_zone bigint NOT NULL REFERENCES tb_zone, name text,
                        deleted_at timestamptz);
  INSERT INTO tb_place (pk_place, name) VALUES (1, 'p1'), (2, 'p2');
  INSERT INTO tb_zone (pk_zone, name) VALUES (1, 'z1');
  INSERT INTO tb_item (pk_item, fk_place, fk_zone, name)
    VALUES (1, 1, 1, 'i1'), (2, 2, 1, 'i2'), (3, 1, 1, 'i3'), (4, 2, 1, 'i4');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('name', i.name, 'place', pc.n) AS data
    FROM tb_item i
    JOIN (SELECT fk_place, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
          GROUP BY fk_place) pc ON pc.fk_place = i.fk_place $$);
  SELECT tviews.pg_tviews_create('tv_zone', $$
    SELECT z.pk_zone, z.id,
           jsonb_build_object('name', z.name,
             'items', jsonb_agg(jsonb_build_object('item', i.pk_item, 'place', pc.n)
                                ORDER BY i.pk_item)) AS data
    FROM tb_zone z
    JOIN tb_item i ON i.fk_zone = z.pk_zone
    JOIN (SELECT fk_place, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
          GROUP BY fk_place) pc ON pc.fk_place = i.fk_place
    GROUP BY z.pk_zone, z.id, z.name $$);
  -- Fires after pg_tviews' delta triggers ("trg_tview_delta_…") and before its
  -- flush ("trg_tview_flush_…"): statement triggers fire in name order.
  CREATE FUNCTION gate() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      PERFORM pg_advisory_xact_lock_shared(277, current_setting('test.gate')::int);
      RETURN NULL;
    END $$;
  CREATE TRIGGER trg_tview_e_gate AFTER UPDATE ON tb_item
    FOR EACH STATEMENT EXECUTE FUNCTION gate();
}

teardown
{
  DROP TABLE tv_zone, tv_item;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_item, tb_zone, tb_place;
  DROP FUNCTION gate();
}

session gate
step g_close   { SELECT pg_advisory_lock(277, 1), pg_advisory_lock(277, 2); }
step g_open1   { SELECT pg_advisory_unlock(277, 1); }
step g_open2   { SELECT pg_advisory_unlock(277, 2); }

session one
setup          { SET deadlock_timeout = '100ms'; SET test.gate = '1'; }
step o_delete  { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 1; }

session two
setup          { SET deadlock_timeout = '10s'; SET test.gate = '2'; }
step t_delete  { UPDATE tb_item SET deleted_at = now() WHERE pk_item = 2; }
step t_check
{
  SELECT 'item' AS tview, t.pk_item AS pk, t.data = v.data AS fresh
  FROM tv_item t FULL JOIN tviews.public__tv_item v USING (pk_item)
  UNION ALL
  SELECT 'zone', t.pk_zone, t.data = v.data
  FROM tv_zone t FULL JOIN tviews.public__tv_zone v USING (pk_zone)
  ORDER BY 1, 2;
}

permutation g_close o_delete t_delete g_open1 g_open2 t_check
permutation g_close t_delete o_delete g_open2 g_open1 t_check
