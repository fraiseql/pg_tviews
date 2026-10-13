# A username write patches the comments on the posts of the user, found through
# tb_post (#228), while another transaction moves a post to that user. Whichever
# commits first, every comment ends with the current name of its post author: the
# repoint recompute waits for the rename value lock on the user, and the
# rename lookup through tb_post sees a repoint committed before it. At
# REPEATABLE READ a repoint the rename snapshot missed fails it with 40001.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        username text NOT NULL);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_author bigint NOT NULL REFERENCES tb_user, title text NOT NULL);
  CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           fk_post bigint NOT NULL REFERENCES tb_post, body text NOT NULL);
  INSERT INTO tb_user (pk_user, username) VALUES (1, 'ann'), (2, 'bob');
  INSERT INTO tb_post (pk_post, fk_author, title) VALUES (1, 1, 'a'), (2, 2, 'b');
  INSERT INTO tb_comment (pk_comment, fk_post, body) VALUES (1, 1, 'x'), (2, 2, 'y');
  SELECT tviews.pg_tviews_create('tv_comment', $$
      SELECT c.pk_comment, c.id, c.fk_post,
             jsonb_build_object('body', c.body,
                 'post', jsonb_build_object('title', p.title,
                     'author', jsonb_build_object('username', u.username))) AS data
      FROM tb_comment c JOIN tb_post p ON p.pk_post = c.fk_post
      JOIN tb_user u ON u.pk_user = p.fk_author $$);
}

teardown
{
  DROP TABLE tv_comment;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_comment, tb_post, tb_user;
}

session r
step r_begin    { BEGIN; }
step r_begin_rr { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT 1 AS snapshot; }
step r_rename   { UPDATE tb_user SET username = 'ann2' WHERE pk_user = 1; }
step r_commit   { COMMIT; }
step r_check
{
  SELECT t.pk_comment, t.data -> 'post' -> 'author' ->> 'username' AS author,
         t.data = v.data AS fresh
  FROM tv_comment t JOIN tviews.public__tv_comment v USING (pk_comment) ORDER BY 1;
}

session m
step m_begin   { BEGIN; }
step m_repoint { UPDATE tb_post SET fk_author = 1 WHERE pk_post = 2; }
step m_commit  { COMMIT; }

# The rename first: the repoint recompute of comment 2 waits for it.
permutation r_begin r_rename m_begin m_repoint r_commit m_commit r_check
# The repoint first: the rename waits for it, and its lookup finds post 2.
permutation m_begin m_repoint r_begin r_rename m_commit r_commit r_check
# REPEATABLE READ, the repoint committed after the rename snapshot: 40001.
permutation r_begin_rr m_begin m_repoint m_commit r_rename r_commit r_check
