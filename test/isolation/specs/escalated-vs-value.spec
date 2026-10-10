# Escalation never weakens a conflict: a writer that locked the whole table
# (past pg_tviews.lock_escalation_threshold, 0 here) still stops a refresh that
# reads one of its values, and a refresh that locked the whole table stops a
# writer of one value. Both end fresh, and the waiter counts its wait.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user bigint NOT NULL REFERENCES tb_user, title text);
  INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'bob');
  INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
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

session inserter
step i_begin     { BEGIN; }
step i_escalate  { SET LOCAL pg_tviews.lock_escalation_threshold = 0; }
step i_insert    { INSERT INTO tb_post (pk_post, fk_user, title) VALUES (2, 1, 'p2'); }
step i_waited    { SELECT (tviews.pg_tviews_queue_stats()->>'value_lock_waits')::int > 0 AS waited; }
step i_commit    { COMMIT; }

session renamer
step r_begin     { BEGIN; }
step r_escalate  { SET LOCAL pg_tviews.lock_escalation_threshold = 0; }
step r_rename    { UPDATE tb_user SET name = 'grace' WHERE pk_user = 1; }
step r_waited    { SELECT (tviews.pg_tviews_queue_stats()->>'value_lock_waits')::int > 0 AS waited; }
step r_commit    { COMMIT; }
step r_check
{
  SELECT t.pk_post, t.data->>'author' AS author, t.data = v.data AS fresh
  FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

permutation r_begin r_escalate r_rename i_begin i_insert r_commit i_waited i_commit r_check
permutation i_begin i_escalate i_insert r_begin r_rename i_commit r_waited r_commit r_check
