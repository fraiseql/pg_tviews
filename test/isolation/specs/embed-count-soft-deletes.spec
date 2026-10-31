# An application schema with an embed fan-out (#277): every item's TVIEW row
# shows the counts of items not deleted in its place and in its group, so
# soft-deleting one item changes a link and rewrites every item of its place
# and of its group. Two transactions each soft-delete an item of the same group,
# in two places. A statement trigger between pg_tviews' delta triggers and its
# flush waits on a gate, one per session, so the writes capture their changes
# one after the other before either refreshes; the gates open one after the
# other. Each refresh reads the other's place. Both commit, and the TVIEW is
# fresh.

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
  INSERT INTO tb_place (pk_place, name) VALUES (1, 'p1'), (2, 'p2');
  INSERT INTO tb_group (pk_group, name) VALUES (1, 'g1');
  INSERT INTO tb_item (pk_item, fk_place, fk_group, name)
    VALUES (1, 1, 1, 'i1'), (2, 2, 1, 'i2'), (3, 1, 1, 'i3'), (4, 2, 1, 'i4');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id,
           jsonb_build_object('name', i.name, 'place', pc.n, 'group', gc.n) AS data
    FROM tb_item i
    JOIN (SELECT fk_place, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
          GROUP BY fk_place) pc ON pc.fk_place = i.fk_place
    JOIN (SELECT fk_group, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
          GROUP BY fk_group) gc ON gc.fk_group = i.fk_group $$);
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
  DROP TABLE tv_item;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_item, tb_group, tb_place;
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
  SELECT t.pk_item, (t.data->>'place')::int AS place, (t.data->>'group')::int AS "group", t.data = v.data AS fresh
  FROM tv_item t FULL JOIN tviews.public__tv_item v USING (pk_item) ORDER BY 1;
}

permutation g_close o_delete t_delete g_open1 g_open2 t_check
permutation g_close o_delete t_delete g_open2 g_open1 t_check
