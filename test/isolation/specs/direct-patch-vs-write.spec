# A direct patch and a recompute of the same TVIEW row, from two writers: the
# second waits on the first's row lock and its document includes the first's
# change once it commits, and none of it once the first rolls back.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text, bio text);
  INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'ada', 'b0');
  SELECT tviews.pg_tviews_create('tv_user', $$
      SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user $$);
}

teardown
{
  DROP TABLE tv_user;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_user;
}

session patcher
step p_begin    { BEGIN; }
step p_patch    { UPDATE tb_user SET bio = 'b1' WHERE pk_user = 1; }
step p_commit   { COMMIT; }
step p_rollback { ROLLBACK; }

session writer
setup           { SET pg_tviews.direct_patch_enabled = off; }
step w_write    { UPDATE tb_user SET name = 'grace' WHERE pk_user = 1; }
step w_check
{
  SELECT t.data AS tview, t.data = v.data AS fresh
  FROM tv_user t JOIN tviews.public__tv_user v USING (pk_user);
}

permutation p_begin p_patch w_write p_commit w_check
permutation p_begin p_patch w_write p_rollback w_check
