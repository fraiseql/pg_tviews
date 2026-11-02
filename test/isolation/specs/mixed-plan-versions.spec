# A TVIEW re-registered (its plan re-derived, its triggers synced) while another
# transaction writes its base tables. Neither waits for the other: the writer
# reads the plan committed when its statements start, the old one until the
# re-registration commits. In every order, after both commit, the TVIEW equals
# its view.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL REFERENCES tb_user, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'bob');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
}

session reg
step r_begin     { BEGIN; }
step r_reregister { SELECT tviews.pg_tviews_reregister('post') IS NOT NULL AS reregistered; }
step r_commit    { COMMIT; }

session writer
step w_begin     { BEGIN; }
step w_rename    { UPDATE tb_user SET name = 'grace' WHERE pk_user = 1; }
step w_insert    { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (3, 1, 'p3'); }
step w_commit    { COMMIT; }
step w_check
{
  SELECT t.pk_post, t.data->>'author' AS author, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation r_begin r_reregister w_begin w_rename w_insert r_commit w_commit w_check
permutation w_begin w_rename r_begin r_reregister w_insert w_commit r_commit w_check
permutation w_begin w_rename w_insert w_commit r_begin r_reregister r_commit w_check
