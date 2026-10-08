# A REPEATABLE READ or SERIALIZABLE writer whose refresh would recompute a TVIEW
# row from a snapshot older than another writer's committed change: it fails
# with a serialization error instead of writing a stale document.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL REFERENCES tb_user, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.name) AS data
      FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session snap
setup          { SET pg_tviews.direct_patch_enabled = off; }
step rr_begin  { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) FROM tb_post; }
step ser_begin { BEGIN ISOLATION LEVEL SERIALIZABLE; SELECT count(*) FROM tb_post; }
step s_title   { UPDATE tb_post SET title = 'p1!' WHERE pk_post = 1; }
step s_commit  { COMMIT; }

session other
step o_rename  { UPDATE tb_user SET name = 'grace' WHERE pk_user = 1; }
step o_check
{
  SELECT t.data AS tview, t.data = v.data AS fresh
  FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post);
}

permutation rr_begin o_rename s_title s_commit o_check
permutation ser_begin o_rename s_title s_commit o_check
