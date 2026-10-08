# A parent row inserted while the child it embeds is renamed: whichever commits
# first, the new parent's document holds the child's committed name.

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
  SELECT tviews.pg_tviews_create('tv_user', $$
      SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
      FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
}

teardown
{
  DROP TABLE tv_post, tv_user;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session inserter
step i_begin  { BEGIN; }
step i_insert { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (2, 1, 'p2'); }
step i_commit { COMMIT; }

session renamer
step r_begin  { BEGIN; }
step r_rename { UPDATE tb_user SET name = 'grace' WHERE pk_user = 1; }
step r_commit { COMMIT; }
step r_check
{
  SELECT t.pk_post, t.data->'author'->>'name' AS author, t.data = v.data AS fresh
  FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation i_begin i_insert r_begin r_rename r_commit i_commit r_check
permutation i_begin i_insert r_begin r_rename i_commit r_commit r_check
permutation r_begin r_rename i_begin i_insert r_commit i_commit r_check
permutation r_begin r_rename i_begin i_insert i_commit r_commit r_check
