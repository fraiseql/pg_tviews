# A post naming a user that doesn't exist yet (an outer join, no foreign key)
# inserted while that user is inserted: once both have committed, the post holds
# the user's name. Nothing exists to lock by row; the join value is locked.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT p.pk_post, p.id, jsonb_build_object('title', p.title, 'author', u.name) AS data
      FROM tb_post p LEFT JOIN tb_user u ON u.pk_user = p.fk_user $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session poster
step p_begin  { BEGIN; }
step p_insert { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (2, 9, 'p2'); }
step p_commit { COMMIT; }

session signup
step s_begin  { BEGIN; }
step s_insert { INSERT INTO tb_user (pk_user, name) VALUES (9, 'ivy'); }
step s_commit { COMMIT; }
step s_check
{
  SELECT t.pk_post, t.data->>'author' AS author, t.data = v.data AS fresh
  FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation p_begin p_insert s_begin s_insert p_commit s_commit s_check
permutation s_begin s_insert p_begin p_insert s_commit p_commit s_check
