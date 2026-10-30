# TRUNCATE of one partition, its rows mapped to the keys they reach, against a
# write into a sibling partition reaching the same TVIEW row: the write's refresh
# reads the partitioned table, waits for the truncated partition's lock, and
# computes the row from what the TRUNCATE committed. Both orders end fresh.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid());
  CREATE TABLE tb_line (pk_line bigint NOT NULL, fk_order bigint NOT NULL, pos int NOT NULL,
                        PRIMARY KEY (pk_line)) PARTITION BY RANGE (pk_line);
  CREATE TABLE tb_line_1 PARTITION OF tb_line FOR VALUES FROM (0) TO (100);
  CREATE TABLE tb_line_2 PARTITION OF tb_line FOR VALUES FROM (100) TO (200);
  INSERT INTO tb_order (pk_order) VALUES (1), (2);
  INSERT INTO tb_line VALUES (1, 1, 1), (2, 2, 1), (110, 1, 2);
  SELECT tviews.pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           jsonb_build_object('lines', (SELECT jsonb_agg(l.pos ORDER BY l.pk_line) FROM tb_line l
                                        WHERE l.fk_order = o.pk_order)) AS data
    FROM tb_order o $$);
}

teardown
{
  DROP TABLE tv_order;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_line, tb_order;
}

session truncater
step t_begin    { BEGIN; }
step t_truncate { TRUNCATE tb_line_1; }
step t_commit   { COMMIT; }

session writer
step w_begin    { BEGIN; }
step w_insert   { INSERT INTO tb_line VALUES (120, 1, 3); }
step w_commit   { COMMIT; }
step w_check
{
  SELECT t.pk_order, t.data->'lines' AS lines, t.data = v.data AS fresh
  FROM tv_order t FULL JOIN tviews.public__tv_order v USING (pk_order) ORDER BY 1;
}

permutation t_begin t_truncate w_begin w_insert t_commit w_commit w_check
permutation w_begin w_insert t_begin t_truncate w_commit t_commit w_check
