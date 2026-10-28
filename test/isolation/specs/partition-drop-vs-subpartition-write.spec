# A partition with sub-partitions is dropped (#261) while another transaction
# writes into one of its sub-partitions directly, which locks neither the
# partition nor the root. The drop locks everything below the partition before
# it reads the rows that leave, so it waits for the writer and takes its row out
# of the TVIEW too.

setup
{
  CREATE EXTENSION IF NOT EXISTS jsonb_delta;
  CREATE EXTENSION pg_tviews;
  CREATE TABLE tb_ev (pk_ev bigint NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      yr int NOT NULL, PRIMARY KEY (pk_ev, yr)) PARTITION BY LIST (yr);
  CREATE TABLE tb_ev_2025 PARTITION OF tb_ev FOR VALUES IN (2025);
  CREATE TABLE tb_ev_2026 PARTITION OF tb_ev FOR VALUES IN (2026) PARTITION BY RANGE (pk_ev);
  CREATE TABLE tb_ev_2026a PARTITION OF tb_ev_2026 FOR VALUES FROM (0) TO (100);
  CREATE TABLE tb_ev_2026b PARTITION OF tb_ev_2026 FOR VALUES FROM (100) TO (200);
  INSERT INTO tb_ev (pk_ev, yr) VALUES (1, 2025), (2, 2026), (150, 2026);
  SELECT tviews.pg_tviews_create('tv_ev', $$
      SELECT pk_ev, id, jsonb_build_object('yr', yr) AS data FROM tb_ev $$);
}

teardown
{
  DROP TABLE tv_ev;
  DROP EXTENSION pg_tviews CASCADE;
  DROP TABLE tb_ev;
}

session a
step a_begin  { BEGIN; }
step a_insert { INSERT INTO tb_ev_2026a (pk_ev, yr) VALUES (3, 2026); }
step a_commit { COMMIT; }

session b
step b_drop { DROP TABLE tb_ev_2026; }
step b_check
{
  SELECT t.pk_ev, t.pk_ev IS NOT DISTINCT FROM v.pk_ev AS fresh
  FROM tv_ev t FULL JOIN tviews.public__tv_ev v USING (pk_ev) ORDER BY 1;
}

permutation a_begin a_insert b_drop a_commit b_check
