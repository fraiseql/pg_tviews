# A child inserted while the parent aggregating it is renamed: both writes
# recompute the same parent row, so the second waits on its row lock and the
# document holds both. This already holds; it guards against regressions.

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
      SELECT u.pk_user, u.id, jsonb_build_object('name', u.name, 'posts', COALESCE(
               (SELECT jsonb_agg(p.title ORDER BY p.pk_post) FROM tb_post p
                WHERE p.fk_user = u.pk_user), '[]'::jsonb)) AS data
      FROM tb_user u $$);
}

teardown
{
  DROP TABLE tv_user;
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
  SELECT t.pk_user, t.data AS tview, t.data = v.data AS fresh
  FROM tv_user t JOIN tviews.public__tv_user v USING (pk_user);
}

permutation i_begin i_insert r_begin r_rename i_commit r_commit r_check
permutation r_begin r_rename i_begin i_insert r_commit i_commit r_check
