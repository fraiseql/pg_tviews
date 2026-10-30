# A fan-out patch (a user renamed, the name written into the posts that copy it)
# while one of those posts is recomputed (its title shown through upper(), which
# no patch can write). The recompute locks the row before computing it (ADR
# 0207): it waits for the patch and computes the new name, or the patch waits for
# the recompute and writes the name over it. A recompute computing before the
# patch committed would write the old name over the patch.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', upper(p.title), 'author', jsonb_build_object('name', u.name)) AS data
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
  SELECT t.pk_post, t.data->>'title' AS title, t.data->'author'->>'name' AS author,
         t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

session title
step t_begin  { BEGIN; }
step t_title  { UPDATE tb_post SET title = 'p1!' WHERE pk_post = 1; }
step t_commit { COMMIT; }

permutation n_begin n_rename t_begin t_title n_commit t_commit n_check
permutation t_begin t_title n_begin n_rename t_commit n_commit n_check
