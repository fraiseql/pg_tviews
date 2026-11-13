# No column skip while row security applies to the TVIEW's owner, also under
# concurrency: a write to the documents table and a refresh of a folder run at
# once, and are ordered whatever column changed. Once both have committed, the
# folder's document is what its owner reads.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE ROLE regress_rlsiso_owner;
  GRANT CREATE ON SCHEMA public TO regress_rlsiso_owner;
  CREATE TABLE tb_folder (pk_folder bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
  CREATE TABLE tb_doc (pk_doc bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_folder bigint NOT NULL REFERENCES tb_folder, title text,
                       active boolean NOT NULL DEFAULT true);
  INSERT INTO tb_folder (pk_folder, name) VALUES (1, 'f1');
  INSERT INTO tb_doc (pk_doc, fk_folder, title) VALUES (1, 1, 'd1'), (2, 1, 'd2');
  ALTER TABLE tb_doc ENABLE ROW LEVEL SECURITY;
  CREATE POLICY doc_active ON tb_doc FOR SELECT TO regress_rlsiso_owner USING (active);
  GRANT SELECT, TRIGGER ON tb_folder, tb_doc TO regress_rlsiso_owner;
  SET ROLE regress_rlsiso_owner;
  SELECT tviews.pg_tviews_create('public.tv_folder', $$
      SELECT f.pk_folder, f.id, jsonb_build_object('name', f.name,
          'docs', (SELECT jsonb_agg(d.title ORDER BY d.pk_doc) FROM public.tb_doc d
                   WHERE d.fk_folder = f.pk_folder)) AS data
      FROM public.tb_folder f $$);
  RESET ROLE;
}

teardown
{
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_doc, tb_folder;
  DROP OWNED BY regress_rlsiso_owner CASCADE;
  DROP ROLE regress_rlsiso_owner;
}

session refresher
step r_begin  { BEGIN; }
step r_rename { UPDATE tb_folder SET name = 'f1b' WHERE pk_folder = 1; }
step r_commit { COMMIT; }

session writer
step w_begin  { BEGIN; }
step w_flip   { UPDATE tb_doc SET active = false WHERE pk_doc = 1; }
step w_commit { COMMIT; }
step w_check
{
  SET ROLE regress_rlsiso_owner;
  SELECT t.data->>'name' AS name, t.data->'docs' AS docs, t.data = v.data AS fresh
  FROM tv_folder t JOIN tviews.public__tv_folder v USING (pk_folder) ORDER BY 1;
  RESET ROLE;
}

# The refresh holds the table's lock: the write waits for it, then refreshes.
permutation r_begin r_rename w_begin w_flip r_commit w_commit w_check
# The write holds the intent lock: the refresh waits for it, then reads anew.
permutation w_begin w_flip r_begin r_rename w_commit r_commit w_check
