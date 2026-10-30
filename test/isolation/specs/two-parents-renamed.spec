# A post copying the names of two parents (its author and its category, each
# written into it by a fan-out patch) while both parents are renamed and the post
# is recomputed (its title shown through upper()), in three transactions. Each
# write meets the others on the post, so whichever order they commit in, the
# post shows both new names and its new title, and no transaction deadlocks.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
  CREATE TABLE tb_category (pk_category bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL, fk_category bigint NOT NULL, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann');
  INSERT INTO tb_category (pk_category, name) VALUES (1, 'news');
  INSERT INTO tb_post (pk_post, fk_user, fk_category, title) VALUES (1, 1, 1, 'p1'), (2, 1, 1, 'p2');
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, p.fk_category,
           jsonb_build_object('title', upper(p.title),
                              'author', jsonb_build_object('name', u.name),
                              'category', jsonb_build_object('name', c.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user
    JOIN tb_category c ON c.pk_category = p.fk_category $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user, tb_category;
}

session user
setup         { SET deadlock_timeout = '100ms'; }
step u_begin  { BEGIN; }
step u_rename { UPDATE tb_user SET name = 'amy' WHERE pk_user = 1; }
step u_commit { COMMIT; }

session category
setup         { SET deadlock_timeout = '100ms'; }
step c_begin  { BEGIN; }
step c_rename { UPDATE tb_category SET name = 'sport' WHERE pk_category = 1; }
step c_commit { COMMIT; }

session title
setup         { SET deadlock_timeout = '100ms'; }
step t_begin  { BEGIN; }
step t_title  { UPDATE tb_post SET title = 'p1!' WHERE pk_post = 1; }
step t_commit { COMMIT; }
step t_check
{
  SELECT t.pk_post, t.data->>'title' AS title, t.data->'author'->>'name' AS author,
         t.data->'category'->>'name' AS category, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation u_begin u_rename c_begin c_rename t_begin t_title u_commit c_commit t_commit t_check
permutation t_begin t_title u_begin u_rename c_begin c_rename t_commit u_commit c_commit t_check
permutation c_begin c_rename t_begin t_title u_begin u_rename c_commit t_commit u_commit t_check
