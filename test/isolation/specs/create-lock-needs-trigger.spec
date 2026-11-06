# Creating a TVIEW locks the tables whose writes refresh it, and only after the
# creator is found to hold TRIGGER on them: a role that may only read a table,
# directly or through another role's view, is refused without queueing for its
# lock, so it never holds off the table's writers.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE ROLE regress_clt_reader;
  GRANT CREATE ON SCHEMA public TO regress_clt_reader;
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
  INSERT INTO tb_item (pk_item, name) VALUES (1, 'one');
  GRANT SELECT ON tb_item TO regress_clt_reader;
  CREATE TABLE tb_secret (pk_secret bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
  INSERT INTO tb_secret (pk_secret, name) VALUES (1, 'one');
  CREATE VIEW v_secret AS SELECT pk_secret, id, name FROM tb_secret;
  GRANT SELECT ON v_secret TO regress_clt_reader;
}

teardown
{
  DROP EXTENSION pg_tviews CASCADE;
  DROP VIEW v_secret;
  DROP TABLE tb_item, tb_secret;
  REVOKE CREATE ON SCHEMA public FROM regress_clt_reader;
  DROP ROLE regress_clt_reader;
}

session reader
setup           { SET ROLE regress_clt_reader; }
step r_table
{
  SELECT tviews.pg_tviews_create('tv_mine', $$
    SELECT i.pk_item AS pk_mine, i.id, jsonb_build_object('n', i.name) AS data FROM tb_item i $$);
}
step r_view
{
  SELECT tviews.pg_tviews_create('tv_mine', $$
    SELECT s.pk_secret AS pk_mine, s.id, jsonb_build_object('n', s.name) AS data FROM v_secret s $$);
}

session writer
step w_begin    { BEGIN; }
step w_item     { INSERT INTO tb_item (pk_item, name) VALUES (2, 'two'); }
step w_secret   { INSERT INTO tb_secret (pk_secret, name) VALUES (2, 'two'); }
step w_commit   { COMMIT; }

permutation w_begin w_item r_table w_commit
permutation w_begin w_secret r_view w_commit
