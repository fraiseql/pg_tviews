# A writer locks the relation a refresh reads, instead of its values, only
# when it may write that relation itself. A writer holding INSERT on posts
# alone, whose statement links many users, keeps one lock per user it links:
# writes to the users it did not link go on, and a write to one it linked
# waits. Past sixteen times pg_tviews.lock_escalation_threshold values of one
# such relation the statement fails with 54000, naming no relation.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE ROLE regress_esc_writer;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL, title text);
  INSERT INTO tb_user (pk_user, name) SELECT g, 'u' || g FROM generate_series(1, 1100) g;
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
  GRANT INSERT ON tb_post TO regress_esc_writer;
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post, tb_user;
  DROP ROLE regress_esc_writer;
}

session writer
setup             { SET ROLE regress_esc_writer; }
step w_begin      { BEGIN; }
step w_link65
{
  INSERT INTO tb_post (pk_post, fk_user, title) SELECT g, g, 'p' FROM generate_series(1, 65) g;
}
step w_link1025
{
  INSERT INTO tb_post (pk_post, fk_user, title) SELECT g, g, 'p' FROM generate_series(1, 1025) g;
}
step w_commit     { COMMIT; }
step w_rollback   { ROLLBACK; }

session other
setup             { SET lock_timeout = '1s'; }
step o_rename_unlinked { UPDATE tb_user SET name = 'renamed' WHERE pk_user = 1050; }
step o_insert_user     { INSERT INTO tb_user (pk_user, name) VALUES (2000, 'new'); }
step o_rename_linked   { UPDATE tb_user SET name = 'renamed' WHERE pk_user = 1; }

permutation w_begin w_link65 o_rename_unlinked o_insert_user o_rename_linked w_commit
permutation w_begin w_link1025 w_rollback
