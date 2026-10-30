# A refresh of keys with no TVIEW row yet, while another transaction creates the
# row, commits, and a third one changes it. Every write of a row holds its key
# lock (ADR 0207), so the third transaction waits for the refresh or the refresh
# for it, and the refresh computes after what it waited for. With a key lock for
# missing rows only, the refresh's upsert waited for the third transaction inside
# ON CONFLICT DO UPDATE and wrote what it computed before the wait (#272).
#
# Keys are the uuid `id` of a DISTINCT ON TVIEW; a refresh takes its key locks in
# hash order, ...04 before ...03.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL, rev int NOT NULL, body text);
  INSERT INTO tb_doc VALUES (5, '00000000-0000-0000-0000-000000000005', 1, 'd5'),
                            (31, '00000000-0000-0000-0000-000000000002', 1, 'c31');
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

# Creates the row of doc ...03.
session creator3
step c3_begin  { BEGIN; }
step c3_create { INSERT INTO tb_doc VALUES (30, '00000000-0000-0000-0000-000000000003', 1, 'c30'); }
step c3_commit { COMMIT; }

# Creates the row of doc ...04.
session creator4
step c4_begin  { BEGIN; }
step c4_create { INSERT INTO tb_doc VALUES (40, '00000000-0000-0000-0000-000000000004', 1, 'c40'); }
step c4_commit { COMMIT; }

# Empties doc ...02 and fills it again: its row is deleted, then created anew.
session refiller
step f_begin  { BEGIN; }
step f_empty2 { DELETE FROM tb_doc WHERE pk_doc = 31; }
step f_fill2  { INSERT INTO tb_doc VALUES (32, '00000000-0000-0000-0000-000000000002', 1, 'c32'); }
step f_commit { COMMIT; }

# Holds the row of doc ...05.
session holder
step h_begin  { BEGIN; }
step h_lock5  { UPDATE tb_doc SET body = 'd5!' WHERE pk_doc = 5; }
step h_commit { COMMIT; }

# Refreshes docs ...03, ...04 and ...05 in one statement (its own rows lose).
session refresher
step r_begin  { BEGIN; }
step r_insert { INSERT INTO tb_doc VALUES (29, '00000000-0000-0000-0000-000000000003', 0, 'r'),
                                          (39, '00000000-0000-0000-0000-000000000004', 0, 'r'),
                                          (49, '00000000-0000-0000-0000-000000000005', 0, 'r'); }
step r_insert25 { INSERT INTO tb_doc VALUES (28, '00000000-0000-0000-0000-000000000002', 0, 'r'),
                                            (48, '00000000-0000-0000-0000-000000000005', 0, 'r'); }
step r_commit { COMMIT; }

# Changes a row once it exists.
session editor
step e_begin  { BEGIN; }
step e_edit3  { UPDATE tb_doc SET body = 'c30!' WHERE pk_doc = 30; }
step e_edit4  { UPDATE tb_doc SET body = 'c40!' WHERE pk_doc = 40; }
step e_delete3 { DELETE FROM tb_doc WHERE pk_doc = 30; }
step e_edit32  { UPDATE tb_doc SET body = 'c32!' WHERE pk_doc = 32; }
step e_commit { COMMIT; }
step e_check
{
  SELECT coalesce(t.id, v.id) AS id, t.data->>'body' AS stored, v.data->>'body' AS computed,
         t.data = v.data AND t.pk_doc = v.pk_doc AS fresh
  FROM tv_doc t FULL JOIN tviews.public__tv_doc v USING (id) ORDER BY 1;
}

# The refresh waits for the row of ...05 while ...03 is created and then
# changed: it holds no lock on ...03 when its upsert reaches it.
permutation c3_begin c3_create h_begin h_lock5 r_begin r_insert c3_commit e_begin e_edit3 h_commit e_commit r_commit e_check
# The refresh waits for the key lock of ...04, then of ...03, while ...04 is
# created: holding ...04's key lock, it makes the change of ...04 wait for it.
permutation c3_begin c3_create c4_begin c4_create r_begin r_insert c4_commit e_begin e_edit4 c3_commit r_commit e_commit e_check
# The same, ...03 deleted instead of changed: the row is gone when the refresh
# locks it, and the refresh writes ...03 from its own row alone.
permutation c3_begin c3_create h_begin h_lock5 r_begin r_insert c3_commit e_begin e_delete3 h_commit e_commit r_commit e_check
# The refresh finds ...02 and waits for its row, which another transaction
# deletes and creates anew; then for ...05, while the new row is changed. The
# row it waited for is gone: ...02 is a key with no row, locked as above.
permutation f_begin f_empty2 h_begin h_lock5 r_begin r_insert25 f_fill2 f_commit e_begin e_edit32 h_commit e_commit r_commit e_check
