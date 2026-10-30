# REPEATABLE READ refreshes and the rows other transactions wrote since their
# snapshot. Another transaction renames the author, which rewrites the post's row
# (a fan-out patch) after the snapshot; a title written into the post from the
# snapshot would undo that, while the base rows don't conflict: the refresh fails
# with 40001. A transaction's own earlier writes are its own, so writing a post
# twice in one transaction never fails.

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

session other
step o_rename { UPDATE tb_user SET name = 'amy' WHERE pk_user = 1; }

session snapshot
step s_begin  { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) AS posts FROM tb_post; }
step s_title  { UPDATE tb_post SET title = 'p1!' WHERE pk_post = 1; }
step s_again  { UPDATE tb_post SET title = 'p1!!' WHERE pk_post = 1; }
step s_commit { COMMIT; }
step s_check
{
  SELECT t.pk_post, t.data->>'title' AS title, t.data->'author'->>'name' AS author,
         t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

# Rewritten after the snapshot: 40001.
permutation s_begin o_rename s_title s_commit s_check
# Its own write, refreshed again in the same transaction: no 40001.
permutation s_begin s_title s_again s_commit s_check
