# Changed TVIEW rows are published on tviews_changes when the writing
# transaction commits (ADR 0231), and only then: a listening session receives
# one message per committed transaction with its net changes, and nothing for a
# rolled-back transaction or savepoint. Ids are fixed so that the payloads print
# the same on every run.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL, name text NOT NULL);
  INSERT INTO tb_user VALUES (1, '00000000-0000-0000-0000-000000000001', 'ann'),
                             (2, '00000000-0000-0000-0000-000000000002', 'bob');
  SELECT tviews.pg_tviews_create('tv_user', $$
      SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM tb_user u $$);
  CREATE TABLE tb_tag (pk_tag bigint PRIMARY KEY, label text NOT NULL);
  INSERT INTO tb_tag VALUES (1, 'x');
  SELECT tviews.pg_tviews_create('tv_tag', $$
      SELECT t.pk_tag, jsonb_build_object('label', t.label) AS data FROM tb_tag t $$);
  CREATE FUNCTION rename_user(p bigint, n text) RETURNS void LANGUAGE sql AS $$
      UPDATE tb_user SET name = n WHERE pk_user = p $$;
}

teardown
{
  DROP FUNCTION rename_user(bigint, text);
  DROP TABLE tv_user, tv_tag;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_user, tb_tag;
}

session l
setup     { LISTEN tviews_changes; }
step l_poll { SELECT 1 AS polled; }

session w
setup              { SET pg_tviews.publish_changes = rows; }
step w_autocommit  { UPDATE tb_user SET name = 'ann2' WHERE pk_user = 1; }
step w_begin       { BEGIN; }
step w_rename_ann  { UPDATE tb_user SET name = 'ann3' WHERE pk_user = 1; }
step w_rename_both { UPDATE tb_user SET name = name || '!'; }
step w_insert      { INSERT INTO tb_user VALUES (3, '00000000-0000-0000-0000-000000000003', 'cy'); }
step w_delete_new  { DELETE FROM tb_user WHERE pk_user = 3; }
step w_delete_bob  { DELETE FROM tb_user WHERE pk_user = 2; }
step w_savepoint   { SAVEPOINT s; }
step w_rollback_to { ROLLBACK TO SAVEPOINT s; }
step w_function    { SELECT rename_user(1, 'ann4'); }
step w_tag         { UPDATE tb_tag SET label = 'y' WHERE pk_tag = 1; }
step w_off         { SET pg_tviews.publish_changes = off; }
step w_commit      { COMMIT; }
step w_rollback    { ROLLBACK; }

# An autocommit write: one message.
permutation w_autocommit l_poll
# Two writes in one transaction: one message, the rows' net changes.
permutation w_begin w_rename_ann w_rename_both l_poll w_commit l_poll
# A rolled-back transaction publishes nothing.
permutation w_begin w_rename_ann w_rollback l_poll
# A rolled-back savepoint's rows are not published; the rest is.
permutation w_begin w_rename_ann w_savepoint w_delete_bob w_rollback_to w_commit l_poll
# A write made by a function a SELECT calls (FraiseQL's mutation shape).
permutation w_function l_poll
# Inserted then deleted: nothing for that row; deleted: in "d".
permutation w_begin w_insert w_delete_new w_delete_bob w_commit l_poll
# A TVIEW without an id column: its type in "t".
permutation w_tag l_poll
# Publishing off: nothing.
permutation w_off w_autocommit l_poll
