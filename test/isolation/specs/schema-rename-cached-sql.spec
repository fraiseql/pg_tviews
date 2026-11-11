# A backend that cached SQL for a TVIEW over tables in schema s1 writes again
# after another backend renamed s1: its cached SQL follows the rename.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE SCHEMA s1;
  CREATE TABLE s1.tb_author (pk_author int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
  CREATE TABLE s1.tb_book (pk_book int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           fk_author int REFERENCES s1.tb_author, title text);
  INSERT INTO s1.tb_author (pk_author, name) VALUES (1, 'a');
  INSERT INTO s1.tb_book (pk_book, fk_author, title) VALUES (1, 1, 't');
  SELECT tviews.pg_tviews_create('public.tv_book', $$
      SELECT b.pk_book, b.id, jsonb_build_object('title', b.title, 'author', a.name) AS data
      FROM s1.tb_book b JOIN s1.tb_author a ON a.pk_author = b.fk_author $$);
}

teardown
{
  DROP TABLE public.tv_book;
  DROP EXTENSION pg_tviews CASCADE;
  DROP SCHEMA IF EXISTS s1 CASCADE;
  DROP SCHEMA IF EXISTS s2 CASCADE;
}

session a
step a_warm   { UPDATE s1.tb_author SET name = 'b' WHERE pk_author = 1; }
step a_write  { UPDATE s2.tb_author SET name = 'c' WHERE pk_author = 1; }
step a_check  { SELECT pk_book, data FROM public.tv_book ORDER BY 1; }

session b
step b_rename { ALTER SCHEMA s1 RENAME TO s2; }

permutation a_warm b_rename a_write a_check
