# A REPEATABLE READ transaction open across a time boundary that writes nothing,
# as pg_dump or a long report holds one, is not waited for: the time refresh
# completes and records its boundary while it is open.

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

session dump
step d_begin    { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) AS rows FROM tv_item; }
step d_read     { SELECT count(*) AS rows FROM tb_item; }
step d_commit   { COMMIT; }

session clock
step c_sleep    { SELECT pg_sleep(1.5); }
step c_refresh  { SELECT * FROM tviews.pg_tviews_refresh_time_dependent(); }
step c_check
{
  SELECT time_boundary IS NOT NULL AS recorded FROM tviews.registry WHERE entity = 'item';
  SELECT pk_item, data->>'current' AS current FROM tv_item ORDER BY 1;
}

permutation d_begin c_sleep c_refresh c_check d_read d_commit
