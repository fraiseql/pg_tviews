# A comment for post 9 inserted while post 9 itself is inserted (no foreign
# key): the comment's table carries the TVIEW key in its row (local), so both
# transactions refresh the same, not yet existing, TVIEW row. Once both have
# committed, the post holds the comment.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           fk_post bigint, body text);
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'p1');
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT p.pk_post, p.id, jsonb_build_object('title', p.title, 'comments', COALESCE(
               (SELECT jsonb_agg(c.body ORDER BY c.pk_comment) FROM tb_comment c
                WHERE c.fk_post = p.pk_post), '[]'::jsonb)) AS data
      FROM tb_post p $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_comment, tb_post;
}

session commenter
step c_begin  { BEGIN; }
step c_insert { INSERT INTO tb_comment (pk_comment, fk_post, body) VALUES (1, 9, 'hi'); }
step c_commit { COMMIT; }

session poster
step p_begin  { BEGIN; }
step p_insert { INSERT INTO tb_post (pk_post, title) VALUES (9, 'p9'); }
step p_commit { COMMIT; }
step p_check
{
  SELECT t.pk_post, t.data->'comments' AS comments, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation c_begin c_insert p_begin p_insert c_commit p_commit p_check
permutation p_begin p_insert c_begin c_insert p_commit c_commit p_check
