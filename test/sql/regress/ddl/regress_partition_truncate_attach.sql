-- TRUNCATE of a partition and ATTACH PARTITION refresh only the keys the
-- partition's rows reach, as a DELETE or an INSERT of those rows would, never the
-- whole TVIEW: retention jobs and partition swaps cost what their rows cost.
-- A partition the TVIEW's owner cannot read whole, a foreign partition, and
-- TRUNCATE of the partitioned table itself still refresh in full.
--
-- Runs in autocommit (no -1): stats merge when each transaction commits.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_partition_truncate_attach.sql
--
-- expect-output: partition truncate and attach: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- tb_order is the entity table; tb_line is partitioned (two levels) and read by
-- fk_order.
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       ref text);
CREATE TABLE tb_line (pk_line bigint NOT NULL, fk_order bigint NOT NULL, pos int NOT NULL,
                      PRIMARY KEY (pk_line)) PARTITION BY RANGE (pk_line);
CREATE TABLE tb_line_1 PARTITION OF tb_line FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_line_2 PARTITION OF tb_line FOR VALUES FROM (100) TO (200)
    PARTITION BY RANGE (pk_line);
CREATE TABLE tb_line_2a PARTITION OF tb_line_2 FOR VALUES FROM (100) TO (150);
CREATE TABLE tb_line_2b PARTITION OF tb_line_2 FOR VALUES FROM (150) TO (200);
CREATE TABLE tb_line_3 PARTITION OF tb_line FOR VALUES FROM (200) TO (300);
INSERT INTO tb_order (pk_order, ref) SELECT g, 'o' || g FROM generate_series(1, 6) g;
INSERT INTO tb_line VALUES (1, 1, 1), (2, 2, 1), (110, 3, 1), (160, 4, 1), (170, 1, 2),
                           (210, 5, 1), (220, 6, 1);

SELECT tviews.pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           jsonb_build_object('ref', o.ref,
               'lines', (SELECT jsonb_agg(l.pos ORDER BY l.pk_line) FROM tb_line l
                         WHERE l.fk_order = o.pk_order)) AS data
    FROM tb_order o $$);

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN tviews.public__tv_order v USING (pk_order)
               WHERE t.pk_order IS NULL OR v.pk_order IS NULL
                  OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'FAIL: %: tv_order differs from its definition', label;
    END IF;
END $$;
CREATE FUNCTION full_refreshes() RETURNS bigint LANGUAGE sql AS $$
    SELECT COALESCE(full_refreshes, 0) FROM tviews.stats WHERE entity = 'order' $$;
CREATE TABLE full_before (n bigint);
CREATE FUNCTION mark() RETURNS void LANGUAGE sql AS $$
    DELETE FROM full_before; INSERT INTO full_before SELECT full_refreshes() $$;
CREATE FUNCTION check_full_refreshes(label text, want bigint) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF full_refreshes() - (SELECT n FROM full_before) <> want THEN
        RAISE EXCEPTION 'FAIL: %: % full refresh(es) of tv_order, expected %', label,
            full_refreshes() - (SELECT n FROM full_before), want;
    END IF;
END $$;

-- ── TRUNCATE of a partition ─────────────────────────────────────────────────
SELECT mark();
TRUNCATE tb_line_1;
SELECT check_fresh('TRUNCATE of a leaf partition');
SELECT check_full_refreshes('TRUNCATE of a leaf partition', 0);

SELECT mark();
TRUNCATE tb_line_2;
SELECT check_fresh('TRUNCATE of a partition with sub-partitions');
SELECT check_full_refreshes('TRUNCATE of a partition with sub-partitions', 0);

-- inside a transaction rolled back, and under a savepoint rolled back
INSERT INTO tb_line VALUES (1, 1, 1), (110, 3, 1);
BEGIN;
TRUNCATE tb_line_1;
ROLLBACK;
SELECT check_fresh('TRUNCATE rolled back');
BEGIN;
SAVEPOINT s;
TRUNCATE tb_line_1;
SELECT check_fresh('TRUNCATE under a savepoint');
ROLLBACK TO SAVEPOINT s;
COMMIT;
SELECT check_fresh('TRUNCATE rolled back to a savepoint');

-- TRUNCATE of a partition locks the partition, as PostgreSQL's does, not its parent.
BEGIN;
TRUNCATE tb_line_1;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_locks WHERE relation = 'tb_line'::regclass
               AND pid = pg_backend_pid() AND mode = 'AccessExclusiveLock') THEN
        RAISE EXCEPTION 'FAIL: TRUNCATE of a partition holds ACCESS EXCLUSIVE on tb_line';
    END IF;
END $$;
ROLLBACK;

-- ── ATTACH PARTITION ────────────────────────────────────────────────────────
CREATE TABLE tb_line_4 (pk_line bigint NOT NULL, fk_order bigint NOT NULL, pos int NOT NULL);
INSERT INTO tb_line_4 VALUES (310, 2, 7), (320, 6, 8);
SELECT mark();
ALTER TABLE tb_line ATTACH PARTITION tb_line_4 FOR VALUES FROM (300) TO (400);
SELECT check_fresh('ATTACH of a populated table');
SELECT check_full_refreshes('ATTACH of a populated table', 0);
-- the attached partition got its triggers: writes to it follow
INSERT INTO tb_line_4 VALUES (330, 3, 9);
SELECT check_fresh('a write to the attached partition');

-- ── still a full refresh ────────────────────────────────────────────────────
SELECT mark();
TRUNCATE tb_line;
SELECT check_fresh('TRUNCATE of the partitioned table');
SELECT check_full_refreshes('TRUNCATE of the partitioned table', 1);

\echo 'partition truncate and attach: PASS'
