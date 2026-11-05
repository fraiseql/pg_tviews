# The lock that serializes registering, changing and dropping one TVIEW is
# private to pg_tviews: an advisory lock any role takes with
# pg_advisory_lock() never delays it, and neither does a role that may not
# change the TVIEW, whose refused call leaves its transaction open. Two calls
# on the same TVIEW still run one after the other.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE ROLE regress_rl_stranger;
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'one');
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM tb_post p $$);
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post;
  DROP ROLE regress_rl_stranger;
}

session stranger
setup           { SET ROLE regress_rl_stranger; }
step s_lock     { SELECT pg_advisory_lock(1953917285, hashtext('post')) IS NULL AS locked; }
step s_unlock   { SELECT pg_advisory_unlock_all(); }
step s_begin    { BEGIN; }
step s_drop     { SELECT tviews.pg_tviews_drop('post'); }
step s_replace
{
  SELECT tviews.pg_tviews_create_or_replace('post', $$
    SELECT p.pk_post, p.id, jsonb_build_object('t', p.title) AS data FROM tb_post p $$);
}
step s_rollback { ROLLBACK; }

session owner
setup           { SET lock_timeout = '1s'; }
step o_begin    { BEGIN; }
step o_rereg    { SELECT tviews.pg_tviews_reregister('post') IS NOT NULL AS reregistered; }
step o_commit   { COMMIT; }

session other
step x_rereg    { SELECT tviews.pg_tviews_reregister('post') IS NOT NULL AS reregistered; }

permutation s_lock o_rereg s_unlock
permutation o_begin o_rereg x_rereg o_commit
permutation s_begin s_drop o_rereg s_rollback
permutation s_begin s_replace o_rereg s_rollback
