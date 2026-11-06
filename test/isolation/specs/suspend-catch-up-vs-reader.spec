# Catching up after suspended refresh, by a writer that is not the TVIEW's
# owner, changes only the rows that differ from the view and takes no lock a
# reader of the TVIEW waits for: the writer holds INSERT on a base table and no
# privilege that would let it lock the TVIEW's table. Its owner's catch-up
# still rebuilds the TVIEW whole (TRUNCATE), and a reader waits for it.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE ROLE regress_cu_writer;
  CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_post (pk_post, title) VALUES (1, 'one'), (2, 'two');
  SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM tb_post p $$);
  GRANT INSERT ON tb_post TO regress_cu_writer;
  GRANT SELECT ON tv_post TO PUBLIC;
}

teardown
{
  DROP TABLE tv_post;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_post;
  DROP ROLE regress_cu_writer;
}

session writer
setup           { SET ROLE regress_cu_writer; }
step w_begin    { BEGIN; }
step w_suspend  { SELECT tviews.pg_tviews_suspend_triggers(); }
step w_insert   { INSERT INTO tb_post (pk_post, title) VALUES (3, 'three'); }
step w_resume   { SELECT tviews.pg_tviews_resume_triggers(); }
step w_exclusive
{
  SELECT count(*) AS exclusive_locks FROM pg_locks
  WHERE pid = pg_backend_pid() AND relation = 'tv_post'::regclass
    AND mode = 'AccessExclusiveLock';
}
step w_commit   { COMMIT; }

session owner
step o_begin    { BEGIN; }
step o_suspend  { SELECT tviews.pg_tviews_suspend_triggers(); }
step o_insert   { INSERT INTO tb_post (pk_post, title) VALUES (4, 'four'); }
step o_resume   { SELECT tviews.pg_tviews_resume_triggers(); }
step o_commit   { COMMIT; }
step o_check
{
  SELECT t.pk_post, t.data->>'title' AS title, t.data = v.data AS fresh
  FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) ORDER BY 1;
}

session reader
step r_read     { SELECT count(*) AS rows FROM tv_post; }

permutation w_begin w_suspend w_insert w_resume w_exclusive r_read w_commit o_check
permutation o_begin o_suspend o_insert o_resume r_read o_commit o_check
