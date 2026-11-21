# The time refresh locks only the rows the boundary changes. A writer of a row the
# boundary leaves as it was goes on while the refresh is open, instead of waiting
# for its transaction behind a lock on every table the TVIEW reads. A writer of a
# row the boundary changes waits for that row only.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text NOT NULL, ends_at timestamptz NOT NULL);
  INSERT INTO tb_item (pk_item, name, ends_at)
  VALUES (1, 'kept', now() + interval '1 day'), (2, 'ending', now() + interval '1 second');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('name', name, 'current', now() < ends_at) AS data
    FROM tb_item $$, jsonb_build_object('time_refresh', 'external'));
}

teardown
{
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_item;
}

session clock
step c_sleep    { SELECT pg_sleep(1.5); }
step c_begin    { BEGIN; }
step c_refresh  { SELECT * FROM tviews.pg_tviews_refresh_time_dependent(); }
step c_commit   { COMMIT; }
step c_check
{
  SELECT pk_item, data->>'name' AS name, data->>'current' AS current FROM tv_item ORDER BY 1;
  SELECT (SELECT jsonb_agg(data ORDER BY pk_item) FROM tv_item)
       = (SELECT jsonb_agg(data ORDER BY pk_item) FROM tviews.public__tv_item) AS fresh;
}

session writer
step w_kept     { UPDATE tb_item SET name = 'renamed' WHERE pk_item = 1; }

permutation c_sleep c_begin c_refresh w_kept c_commit c_check
