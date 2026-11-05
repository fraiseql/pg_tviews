# The owner a rebuild analyzes a TVIEW as holds for that rebuild only: when the
# rebuild fails (here, its lock wait times out), a later create in the same
# backend is analyzed as its own creator and filled.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE ROLE regress_rbo_owner;
  CREATE ROLE regress_rbo_creator;
  GRANT CREATE ON SCHEMA public TO regress_rbo_owner, regress_rbo_creator;
  SET ROLE regress_rbo_owner;
  CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        title text);
  INSERT INTO tb_item (pk_item, title) VALUES (1, 'a');
  CREATE TABLE tb_other (pk_other bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_item bigint UNIQUE, v text);
  INSERT INTO tb_other (pk_other, fk_item, v) VALUES (1, 1, 'v1');
  SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('title', title) AS data FROM tb_item $$);
  CREATE TABLE owner_private (s text);
  INSERT INTO owner_private VALUES ('private');
  CREATE VIEW owner_view AS SELECT s FROM owner_private;
  SET ROLE regress_rbo_creator;
  CREATE TABLE tb_mine (pk_mine bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        x text);
  INSERT INTO tb_mine (pk_mine, x) VALUES (1, 'mine');
  RESET ROLE;
}

teardown
{
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE IF EXISTS tb_item, tb_other, tb_mine, tv_item, tv_mine, tv_peek CASCADE;
  DROP VIEW owner_view;
  DROP TABLE owner_private;
  REVOKE CREATE ON SCHEMA public FROM regress_rbo_owner, regress_rbo_creator;
  DROP ROLE regress_rbo_owner;
  DROP ROLE regress_rbo_creator;
}

session writer
step w_begin    { BEGIN; }
step w_other    { INSERT INTO tb_other (pk_other, fk_item, v) VALUES (2, NULL, 'v2'); }
step w_sleep    { SELECT pg_sleep(1); }
step w_commit   { COMMIT; }

session admin
step a_rebuild
{
  SET lock_timeout = '200ms';
  SELECT tviews.pg_tviews_create_or_replace('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('title', i.title) AS data, o.v AS extra
    FROM tb_item i LEFT JOIN tb_other o ON o.fk_item = i.pk_item $$);
  RESET lock_timeout;
}
step a_create
{
  SET ROLE regress_rbo_creator;
  SELECT tviews.pg_tviews_create('tv_mine', $$
    SELECT m.pk_mine, m.id, jsonb_build_object('x', m.x) AS data FROM tb_mine m $$);
  SELECT count(*) AS tv_mine_rows FROM tv_mine;
  RESET ROLE;
}
step a_create_private
{
  SET ROLE regress_rbo_creator;
  SELECT tviews.pg_tviews_create('tv_peek', $$
    SELECT m.pk_mine AS pk_peek, m.id,
           jsonb_build_object('s', (SELECT s FROM owner_view LIMIT 1)) AS data
    FROM tb_mine m $$);
}
step a_reset    { RESET ROLE; }

permutation w_begin w_other a_rebuild w_sleep w_commit a_create a_create_private a_reset
