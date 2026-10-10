# Two transactions each insert a post of one user, then rename the other's
# user: each waits for the other's value lock. PostgreSQL detects the deadlock
# and fails one of them with 40P01 (retry it); the other commits, and the TVIEW
# is fresh. The second session checks for deadlocks later, so the first one is
# the one that fails.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL REFERENCES tb_user, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'bob');
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT p.pk_post, p.id, jsonb_build_object('title', p.title, 'author', u.name) AS data
      FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session one
setup          { SET deadlock_timeout = '100ms'; }
step o_begin   { BEGIN; }
step o_insert  { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'); }
step o_rename  { UPDATE tb_user SET name = 'bea' WHERE pk_user = 2; }
step o_commit  { COMMIT; }

session two
setup          { SET deadlock_timeout = '10s'; }
step t_begin   { BEGIN; }
step t_insert  { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (2, 2, 'p2'); }
step t_rename  { UPDATE tb_user SET name = 'amy' WHERE pk_user = 1; }
step t_commit  { COMMIT; }
step t_check
{
  SELECT t.pk_post, t.data->>'author' AS author, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation o_begin o_insert t_begin t_insert o_rename t_rename o_commit t_commit t_check
