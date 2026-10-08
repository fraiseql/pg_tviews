# A TVIEW replaced while a writer changes one of its base tables: the write waits
# for the replacement and is applied under the new definition.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'p1'), (2, 'p2');
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post;
}

session replacer
step x_begin   { BEGIN; }
step x_replace
{
  SELECT tviews.pg_tviews_create_or_replace('public.tv_post', $$
      SELECT pk_post, id, jsonb_build_object('title', upper(title)) AS data FROM tb_post $$);
}
step x_commit  { COMMIT; }

session writer
step w_write   { UPDATE tb_post SET title = 'changed' WHERE pk_post = 1; }
step w_check
{
  SELECT t.pk_post, t.data AS tview, t.data = v.data AS fresh
  FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation x_begin x_replace w_write x_commit w_check
permutation w_write x_begin x_replace x_commit w_check
