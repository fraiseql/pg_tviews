# A transaction that began before a time boundary and wrote a time-dependent
# TVIEW's row before the boundary was announced holds the TVIEW's boundary lock:
# the time refresh waits for it to end, then reads its row and brings it to the
# boundary. Nothing is handed off. The same holds when the announcement lands
# between the writer's last flush and its commit: the lock is held to the commit.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text NOT NULL, ends_at timestamptz NOT NULL);
  INSERT INTO tb_item (pk_item, name, ends_at)
  VALUES (1, 'ending', now() + interval '1 second');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('name', name, 'current', now() < ends_at) AS data
    FROM tb_item $$, jsonb_build_object('time_refresh', 'external'));
  CREATE FUNCTION wait_drained() RETURNS boolean LANGUAGE plpgsql AS $f$
  BEGIN
    FOR i IN 1..100 LOOP
      IF to_regclass('tviews.pg_tview_time_handoff') IS NULL THEN RETURN true; END IF;
      IF NOT EXISTS (SELECT 1 FROM tviews.pg_tview_time_handoff) THEN RETURN true; END IF;
      PERFORM pg_sleep(0.1);
    END LOOP;
    RETURN false;
  END $f$;
}

teardown
{
  DROP TABLE tv_item;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_item;
  DROP FUNCTION wait_drained();
}

session writer
step w_begin    { BEGIN; SELECT 1 AS began; }
step w_insert
{
  INSERT INTO tb_item (pk_item, name, ends_at)
  SELECT 2, 'inserted', ends_at FROM tb_item WHERE pk_item = 1;
}
step w_commit   { COMMIT; }

session clock
step c_sleep    { SELECT pg_sleep(1.5); }
step c_refresh  { SELECT * FROM tviews.pg_tviews_refresh_time_dependent(); }
step c_check
{
  SELECT count(*) AS handed_off FROM tviews.pg_tview_time_handoff;
  SELECT pk_item, data->>'name' AS name, data->>'current' AS current FROM tv_item ORDER BY 1;
  SELECT (SELECT jsonb_agg(data ORDER BY pk_item) FROM tv_item)
       = (SELECT jsonb_agg(data ORDER BY pk_item) FROM tviews.public__tv_item) AS fresh;
}

permutation w_begin c_sleep w_insert c_refresh w_commit c_check
