# A fan-out patch (a user renamed, written into the posts that copy the name)
# while one of those posts is re-pointed to another user. The patch waits for an
# open re-point and then leaves the moved post to it (it writes only rows still
# holding the user), or the re-point waits for the patch and recomputes the post
# after it.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann'), (2, 'bob');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2');
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session rename
step n_begin  { BEGIN; }
step n_rename { UPDATE tb_user SET name = 'amy' WHERE pk_user = 1; }
step n_commit { COMMIT; }
step n_check
{
  SELECT t.pk_post, t.data->'author'->>'name' AS author, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

session repoint
step m_begin  { BEGIN; }
step m_move   { UPDATE tb_post SET fk_user = 2 WHERE pk_post = 1; }
step m_commit { COMMIT; }

permutation m_begin m_move n_begin n_rename m_commit n_commit n_check
permutation n_begin n_rename m_begin m_move n_commit m_commit n_check
