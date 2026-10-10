# An existing parent re-pointed to another child (`UPDATE tb_post SET fk_user`)
# while that child is renamed: once both have committed, the parent's document
# holds the new child's committed name, whether it embeds the child's TVIEW
# (tv_post) or joins its base table (tv_note).

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL REFERENCES tb_user, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'bob');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (3, 2, 'p3');
  SELECT tviews.pg_tviews_create('tv_user', $$
      SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
  SELECT tviews.pg_tviews_create('tv_post', $$
      SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
      FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
  SELECT tviews.pg_tviews_create('tv_note', $$
      SELECT p.pk_post AS pk_note, p.id, jsonb_build_object('author', upper(u.name)) AS data
      FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
}

teardown
{
  DROP TABLE tv_note, tv_post, tv_user;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session repointer
step i_begin   { BEGIN; }
step i_repoint { UPDATE tb_post SET fk_user = 1 WHERE pk_post = 3; }
step i_commit  { COMMIT; }

session renamer
step r_begin  { BEGIN; }
step r_rename { UPDATE tb_user SET name = 'grace' WHERE pk_user = 1; }
step r_commit { COMMIT; }
step r_check
{
  SELECT 'post' AS tv, t.pk_post AS pk, t.data->'author'->>'name' AS author, t.data = v.data AS fresh
    FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post)
  UNION ALL
  SELECT 'note', t.pk_note, t.data->>'author', t.data = v.data
    FROM tv_note t JOIN tviews.public__tv_note v USING (pk_note)
  ORDER BY 1, 2;
}

permutation i_begin i_repoint r_begin r_rename i_commit r_commit r_check
permutation r_begin r_rename i_begin i_repoint r_commit i_commit r_check
