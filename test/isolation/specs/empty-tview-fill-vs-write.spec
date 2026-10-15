# Two transactions writing first into a TVIEW that a crash emptied while its
# view has rows (TRUNCATE plus deleting its row in pg_tview_valid stands in for
# the crash): the first one claims and fills it from the view; the second waits
# for the claim and then finds the TVIEW filled, instead of filling it a second
# time and failing on a duplicate key (#214). Under REPEATABLE READ the second
# fails with 40001, which can be retried.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'p1'), (2, 'p2');
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post $$,
      jsonb_build_object('logged', false));
  TRUNCATE tv_post;
  DELETE FROM tviews.pg_tview_valid;
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post;
}

session a
step a_begin  { BEGIN; }
step a_insert { INSERT INTO tb_post (pk_post, title) VALUES (3, 'p3'); }
step a_commit { COMMIT; }

session b
step b_begin  { BEGIN; }
step b_rr     { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT 1; }
step b_insert { INSERT INTO tb_post (pk_post, title) VALUES (4, 'p4'); }
step b_commit { COMMIT; }
step b_check
{
  SELECT t.pk_post, t.data->>'title' AS title, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation a_begin a_insert b_begin b_insert a_commit b_commit b_check
permutation a_begin a_insert b_rr b_insert a_commit b_commit b_check
