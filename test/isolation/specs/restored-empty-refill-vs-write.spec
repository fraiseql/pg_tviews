# Two transactions writing first into a LOGGED TVIEW restored without its rows
# (TRUNCATE stands in for the restore: the table has no page, and nothing marks
# it untrusted). The first finds it empty, refreshes its row, finds the view's
# other rows missing and refills it. The second sees none of those rows yet:
# it refills too, waits for the first's rows where they meet, and keeps them.
# Every row ends as its view computes it.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'p1'), (2, 'p2');
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post $$);
  TRUNCATE tv_post;
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
step b_insert { INSERT INTO tb_post (pk_post, title) VALUES (4, 'p4'); }
step b_update { UPDATE tb_post SET title = 'p1!' WHERE pk_post = 1; }
step b_commit { COMMIT; }
step b_check
{
  SELECT t.pk_post, t.data->>'title' AS title, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation a_begin a_insert b_begin b_insert a_commit b_commit b_check
permutation a_begin a_insert b_begin b_update a_commit b_commit b_check
permutation b_begin b_update a_begin a_insert b_commit a_commit b_check
