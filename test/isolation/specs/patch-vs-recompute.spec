# A recompute of a TVIEW row while a direct patch of the same row is uncommitted
# (a title written into the post, while the post is re-pointed to another
# author). The recompute locks the row before computing it (ADR 0207): it waits
# for the patch and computes after its commit, or the patch waits for the
# recompute. Either way the post shows its title and its author.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann'), (2, 'bob');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
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

session patch
step p_begin  { BEGIN; }
step p_title  { UPDATE tb_post SET title = 'p1!' WHERE pk_post = 1; }
step p_commit { COMMIT; }

session recompute
step r_begin  { BEGIN; }
step r_move   { UPDATE tb_post SET fk_user = 2 WHERE pk_post = 1; }
step r_commit { COMMIT; }
step r_check
{
  SELECT t.pk_post, t.data->>'title' AS title, t.data->'author'->>'name' AS author,
         t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation p_begin p_title r_begin r_move p_commit r_commit r_check
permutation r_begin r_move p_begin p_title r_commit p_commit r_check
