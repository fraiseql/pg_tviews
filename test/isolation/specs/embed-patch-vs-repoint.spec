# A rename of a user, written into the posts embedding it as a patch, against a
# post re-pointed to another user meanwhile. The rename finds the post among
# the user's parents, then waits for the re-point's row lock; once that commits,
# the post embeds the other user, and the rename's patch must not write the old
# user's name into it: the post is recomputed instead.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_author bigint NOT NULL REFERENCES tb_user, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'bob');
  INSERT INTO tb_post (pk_post, fk_author, title) VALUES (10, 1, 'p10');
  SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM tb_user u $$);
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_author $$);
}

teardown
{
  DROP TABLE tv_post, tv_user;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session repoint
step r_begin   { BEGIN; }
step r_move    { UPDATE tb_post SET fk_author = 2 WHERE pk_post = 10; }
step r_commit  { COMMIT; }

session rename
step n_rename  { UPDATE tb_user SET name = 'amy' WHERE pk_user = 1; }
step n_check
{
  SELECT t.pk_post, t.data->'author'->>'name' AS author, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation r_begin r_move n_rename r_commit n_check
