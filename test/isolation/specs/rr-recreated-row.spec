# A REPEATABLE READ refresh of a TVIEW row that a concurrent transaction deleted
# after the snapshot (a doc whose last revision was deleted). The refresh still
# sees the row, so it takes no key lock (ADR 0207's lock of rows being created),
# and its upsert, finding the row gone, creates it anew. A READ COMMITTED writer
# creating the same row meanwhile found it missing, took the free key lock, and
# then waited inside its own upsert for the new row and wrote what it computed
# before the wait: the REPEATABLE READ write was lost. A REPEATABLE READ refresh
# that would create a row its snapshot sees fails with 40001 instead, as an
# UPDATE of a concurrently deleted row does. So does SERIALIZABLE. A row its
# snapshot does not see at all (a new doc) is created under the key lock, and the
# READ COMMITTED writer waits for it.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL, rev int NOT NULL, body text);
  INSERT INTO tb_doc VALUES (41, '00000000-0000-0000-0000-000000000002', 7, 'b41');
  SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT DISTINCT ON (d.id) d.pk_doc, d.id, jsonb_build_object('body', d.body, 'rev', d.rev) AS data
    FROM tb_doc d ORDER BY d.id, d.rev DESC, d.pk_doc DESC $$);
}

teardown
{
  DROP TABLE tv_doc;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_doc;
}

# Deletes the doc's last revision: its row goes.
session deleter
step d_delete { DELETE FROM tb_doc WHERE pk_doc = 41; }

# REPEATABLE READ, snapshot taken before the delete.
session snapshot
step s_begin  { BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) AS docs FROM tb_doc; }
step s_begin_ser { BEGIN ISOLATION LEVEL SERIALIZABLE; SELECT count(*) AS docs FROM tb_doc; }
step s_new    { INSERT INTO tb_doc VALUES (90, '00000000-0000-0000-0000-000000000009', 1, 'b90'); }
step s_insert { INSERT INTO tb_doc VALUES (47, '00000000-0000-0000-0000-000000000002', 8, 'b47'); }
step s_commit { COMMIT; }

# READ COMMITTED, creates the row too.
session creator
step c_insert { INSERT INTO tb_doc VALUES (48, '00000000-0000-0000-0000-000000000002', 1, 'b48'); }
step c_new    { INSERT INTO tb_doc VALUES (91, '00000000-0000-0000-0000-000000000009', 0, 'b91'); }
step c_check
{
  SELECT coalesce(t.id, v.id) AS id, t.data->>'body' AS stored, v.data->>'body' AS computed,
         t.data = v.data AND t.pk_doc = v.pk_doc AS fresh
  FROM tv_doc t FULL JOIN tviews.public__tv_doc v USING (id) ORDER BY 1;
}

permutation s_begin d_delete s_insert c_insert s_commit c_check
permutation s_begin_ser d_delete s_insert c_insert s_commit c_check
permutation s_begin s_new c_new s_commit c_check
