# A post inserted for user 1 while user 1 is renamed (READ COMMITTED). The
# rename's fan-out finds the posts it patches before the new post is visible, and
# the insert computes its post from user 1's name. The insert changes a link, so
# its refresh takes a shared lock on user 1's value, which the rename's write holds
# exclusively (ADR 0207): it computes after the rename commits, or the rename finds
# the post once the insert commits. Either way the post shows the new name.

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

session rename
step r_begin  { BEGIN; }
step r_rename { UPDATE tb_user SET name = 'amy' WHERE pk_user = 1; }
step r_commit { COMMIT; }
step r_check
{
  SELECT t.pk_post, t.data->'author'->>'name' AS author, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

session insert
step i_begin  { BEGIN; }
step i_insert { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (2, 1, 'p2'); }
step i_other  { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (3, 2, 'p3'); }
step i_commit { COMMIT; }

# The rename first: the insert computes after it.
permutation r_begin r_rename i_begin i_insert r_commit i_commit r_check
# The insert first: the rename's fan-out waits for it, then finds the new post.
permutation i_begin i_insert r_begin r_rename i_commit r_commit r_check
# A post of another user never waits for the rename.
permutation r_begin r_rename i_begin i_other i_commit r_commit r_check
