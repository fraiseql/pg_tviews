# The collection keys of a change (payload v2, "c") name the parents a row
# belonged to before the write and after it, as committed: a comment moved by
# two transactions at once, the second waiting for the first, publishes the
# post the first moved it to as its old parent, not the post its own snapshot
# started from. A cache that tags composed results by collection then evicts
# every post the comment left or joined.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL, title text NOT NULL);
  INSERT INTO tb_post VALUES (1, '00000000-0000-0000-0000-000000000001', 'one'),
                             (2, '00000000-0000-0000-0000-000000000002', 'two'),
                             (3, '00000000-0000-0000-0000-000000000003', 'three');
  CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL,
                           fk_post bigint NOT NULL REFERENCES tb_post, content text NOT NULL);
  INSERT INTO tb_comment VALUES (1, '00000000-0000-0000-0000-0000000000c1', 1, 'hi');
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM tb_post p $$);
  SELECT tviews.pg_tviews_create('tv_comment', $$
      SELECT c.pk_comment, c.id, c.fk_post, jsonb_build_object('content', c.content) AS data
      FROM tb_comment c $$);
}

teardown
{
  DROP TABLE tv_comment, tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_comment, tb_post;
}

session l
setup     { LISTEN tviews_changes; }
step l_poll { SELECT 1 AS polled; }

session a
setup         { SET pg_tviews.publish_changes = rows; }
step a_begin  { BEGIN; }
step a_move   { UPDATE tb_comment SET fk_post = 2 WHERE pk_comment = 1; }
step a_commit { COMMIT; }

session b
setup        { SET pg_tviews.publish_changes = rows; }
step b_begin { BEGIN; }
step b_move  { UPDATE tb_comment SET fk_post = 3 WHERE pk_comment = 1; }
step b_commit { COMMIT; }

# b waits for a's row; once a commits, b moves the comment from post 2, where a
# left it: a names posts 2 and 1, b posts 3 and 2.
permutation a_begin a_move b_begin b_move a_commit b_commit l_poll
