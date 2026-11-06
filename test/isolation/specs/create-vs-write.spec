# A TVIEW created while a write to a table it reads is in flight waits for that
# write and fills from the rows it commits: the new TVIEW starts as its view.
# Both when it is the first TVIEW reading the table and when another one has
# already put triggers on it.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'one'), (2, 'two');
}

teardown
{
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE IF EXISTS tv_post, tv_other;
  DROP TABLE tb_post;
}

session writer
step w_begin    { BEGIN; }
step w_update   { UPDATE tb_post SET title = 'second' WHERE pk_post = 2; }
step w_commit   { COMMIT; }

session creator
step c_other
{
  SELECT tviews.pg_tviews_create('tv_other', $$
    SELECT p.pk_post AS pk_other, p.id, jsonb_build_object('t', p.title) AS data FROM tb_post p $$);
}
step c_create
{
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM tb_post p $$);
}
step c_check
{
  SELECT NOT EXISTS (SELECT 1 FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post)
                     WHERE t.data IS DISTINCT FROM v.data) AS in_sync;
}

permutation w_begin w_update c_create w_commit c_check
permutation c_other w_begin w_update c_create w_commit c_check
