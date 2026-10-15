# Two transactions adding the first orders of a user to an aggregate TVIEW
# (one row per user, no row for that user yet): both create the same TVIEW
# row. The second waits for the first and then counts both orders.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_user bigint NOT NULL, total int);
  INSERT INTO tb_order (pk_order, fk_user, total) VALUES (1, 1, 5);
  SELECT tviews.pg_tviews_create('tv_user_summary', $$
      SELECT o.fk_user AS pk_user_summary, md5(o.fk_user::text)::uuid AS id,
             jsonb_build_object('orders', count(*), 'total', sum(o.total)) AS data
      FROM tb_order o GROUP BY o.fk_user $$,
      jsonb_build_object('group_keys', jsonb_build_object('tb_order', 'fk_user')));
}

teardown
{
  DROP TABLE tv_user_summary;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_order;
}

session a
step a_begin  { BEGIN; }
step a_insert { INSERT INTO tb_order (pk_order, fk_user, total) VALUES (2, 7, 10); }
step a_commit { COMMIT; }

session b
step b_begin  { BEGIN; }
step b_insert { INSERT INTO tb_order (pk_order, fk_user, total) VALUES (3, 7, 20); }
step b_commit { COMMIT; }
step b_check
{
  SELECT t.pk_user_summary, t.data, t.data = v.data AS fresh
  FROM tv_user_summary t FULL JOIN tviews.public__tv_user_summary v USING (pk_user_summary)
  ORDER BY 1;
}

permutation a_begin a_insert b_begin b_insert a_commit b_commit b_check
