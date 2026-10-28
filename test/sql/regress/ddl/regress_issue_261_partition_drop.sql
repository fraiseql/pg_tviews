-- #261: a partition dropped with DROP TABLE takes its rows out of every TVIEW over
-- the partitioned root, in the dropping transaction, as DETACH does. The rows
-- leave as a delete of the partition's rows: only the keys they reach are
-- refreshed, never the whole TVIEW (the cost follows the dropped partition, not
-- what remains). Plain DETACH leaves the same way; DETACH CONCURRENTLY, which
-- commits in the middle, keeps its full refresh.
--
-- Runs in autocommit on purpose (no -1): DETACH CONCURRENTLY cannot run in a
-- transaction block, and freshness is checked after each statement.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_issue_261_partition_drop.sql
--
-- expect-output: partition drop: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'regress_261_owner') THEN
        DROP OWNED BY regress_261_owner, regress_261_tview CASCADE;
    END IF;
END $$;
DROP ROLE IF EXISTS regress_261_owner;
DROP ROLE IF EXISTS regress_261_tview;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- tb_order is the partitioned entity table, tb_line is partitioned and local
-- (read by fk_order), tb_note is partitioned, sub-partitioned and mapped (read
-- through tb_line).
CREATE TABLE tb_order (pk_order bigint NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       ref text, PRIMARY KEY (pk_order)) PARTITION BY RANGE (pk_order);
CREATE TABLE tb_order_1 PARTITION OF tb_order FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_order_2 PARTITION OF tb_order FOR VALUES FROM (100) TO (200);
CREATE TABLE tb_order_3 PARTITION OF tb_order FOR VALUES FROM (200) TO (300);
CREATE TABLE tb_order_4 PARTITION OF tb_order FOR VALUES FROM (300) TO (400);
CREATE TABLE tb_order_5 PARTITION OF tb_order FOR VALUES FROM (400) TO (500);
CREATE TABLE tb_order_6 PARTITION OF tb_order FOR VALUES FROM (500) TO (600);
CREATE TABLE tb_line (pk_line bigint NOT NULL, fk_order bigint NOT NULL, pos int NOT NULL,
                      PRIMARY KEY (pk_line)) PARTITION BY RANGE (pk_line);
CREATE TABLE tb_line_1 PARTITION OF tb_line FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_line_2 PARTITION OF tb_line FOR VALUES FROM (100) TO (200);
CREATE TABLE tb_line_3 PARTITION OF tb_line FOR VALUES FROM (200) TO (300);
CREATE TABLE tb_note (pk_note bigint NOT NULL, fk_line bigint NOT NULL, body text,
                      PRIMARY KEY (pk_note)) PARTITION BY RANGE (pk_note);
CREATE TABLE tb_note_1 PARTITION OF tb_note FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_note_2 PARTITION OF tb_note FOR VALUES FROM (100) TO (200)
    PARTITION BY RANGE (pk_note);
CREATE TABLE tb_note_2a PARTITION OF tb_note_2 FOR VALUES FROM (100) TO (150);
CREATE TABLE tb_note_2b PARTITION OF tb_note_2 FOR VALUES FROM (150) TO (200);

INSERT INTO tb_order (pk_order, ref)
SELECT n, 'o' || n FROM unnest(ARRAY[1, 2, 150, 250, 350, 450, 550]) n;
INSERT INTO tb_line (pk_line, fk_order, pos) VALUES
    (1, 150, 1), (2, 250, 1), (120, 150, 2), (130, 350, 1), (220, 450, 1), (230, 550, 1);
INSERT INTO tb_note (pk_note, fk_line, body) VALUES
    (1, 1, 'n1'), (120, 2, 'n2'), (170, 120, 'n3'), (180, 130, 'n4');

SELECT pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           jsonb_build_object(
               'ref', o.ref,
               'lines', (SELECT jsonb_agg(l.pos ORDER BY l.pk_line) FROM tb_line l
                         WHERE l.fk_order = o.pk_order),
               'notes', (SELECT jsonb_agg(n.body ORDER BY n.body) FROM tb_line l2
                         JOIN tb_note n ON n.fk_line = l2.pk_line WHERE l2.fk_order = o.pk_order)
           ) AS data
    FROM tb_order o $$);

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN tviews.public__tv_order v USING (pk_order)
               WHERE t.pk_order IS NULL OR v.pk_order IS NULL
                  OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'FAIL: %: tv_order has % rows, its definition %', label,
            (SELECT count(*) FROM tv_order), (SELECT count(*) FROM tviews.public__tv_order);
    END IF;
END $$;

-- Full refreshes of tv_order so far (merged when each transaction commits).
CREATE FUNCTION full_refreshes() RETURNS bigint LANGUAGE sql AS $$
    SELECT COALESCE(full_refreshes, 0) FROM tviews.stats WHERE entity = 'order' $$;
CREATE TABLE full_before AS SELECT full_refreshes() AS n;
CREATE FUNCTION check_no_full_refresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF full_refreshes() <> (SELECT n FROM full_before) THEN
        RAISE EXCEPTION 'FAIL: %: tv_order was refreshed in full', label;
    END IF;
END $$;

SELECT check_fresh('after creation');

-- ── DROP TABLE of a partition ───────────────────────────────────────────────
DROP TABLE tb_order_1;
SELECT check_fresh('drop a partition of the entity table');
DROP TABLE tb_line_1;
SELECT check_fresh('drop a partition of a local table');
DROP TABLE tb_note_2b;
SELECT check_fresh('drop a sub-partition of a mapped table');
DROP TABLE tb_note_2;
SELECT check_fresh('drop a middle level of a mapped table');
DROP TABLE tb_order_3, tb_line_3;
SELECT check_fresh('drop two partitions of two tables in one statement');
SELECT check_no_full_refresh('DROP TABLE of partitions');

-- ── inside a transaction ────────────────────────────────────────────────────
BEGIN;
DROP TABLE tb_order_4;
SELECT check_fresh('drop inside a transaction');
ROLLBACK;
SELECT check_fresh('drop rolled back');
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM tv_order WHERE pk_order = 350) THEN
        RAISE EXCEPTION 'FAIL: a rolled-back drop removed rows from tv_order';
    END IF;
END $$;
BEGIN;
SAVEPOINT s;
DROP TABLE tb_order_4;
ROLLBACK TO SAVEPOINT s;
SELECT check_fresh('drop rolled back to a savepoint');
COMMIT;
SELECT check_fresh('after the savepoint transaction');

-- ── DETACH ──────────────────────────────────────────────────────────────────
ALTER TABLE tb_order DETACH PARTITION tb_order_4;
SELECT check_fresh('detach');
SELECT check_no_full_refresh('DETACH');
ALTER TABLE tb_order DETACH PARTITION tb_order_5 CONCURRENTLY;
SELECT check_fresh('detach concurrently');
DO $$ BEGIN
    IF full_refreshes() <= (SELECT n FROM full_before) THEN
        RAISE EXCEPTION 'FAIL: the full-refresh counter did not see DETACH CONCURRENTLY';
    END IF;
END $$;
ALTER TABLE tb_order ATTACH PARTITION tb_order_5 FOR VALUES FROM (400) TO (500);
SELECT check_fresh('attach back');

-- A detached partition is a plain table: dropping it changes no TVIEW.
DROP TABLE tb_order_4;
SELECT check_fresh('drop a detached partition');

-- ── a partition its TVIEW's owner cannot read ───────────────────────────────
-- Partitions keep their own privileges: the TVIEW's owner reads the root, not
-- the partition, so the leaving rows cannot be mapped and the TVIEW is refreshed
-- in full. A role that does not own the partition fails before anything changes.
CREATE ROLE regress_261_owner;
CREATE ROLE regress_261_tview;
GRANT CREATE ON SCHEMA public TO regress_261_owner, regress_261_tview;
SET ROLE regress_261_owner;
CREATE TABLE tb_ev (pk_ev bigint NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(),
                    day date NOT NULL, amount int, PRIMARY KEY (pk_ev, day)) PARTITION BY RANGE (day);
CREATE TABLE tb_ev_2025 PARTITION OF tb_ev FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
CREATE TABLE tb_ev_2026 PARTITION OF tb_ev FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
INSERT INTO tb_ev (pk_ev, day, amount) VALUES (1, '2025-06-01', 10), (2, '2026-06-01', 20);
GRANT SELECT, TRIGGER ON tb_ev TO regress_261_tview;
GRANT TRIGGER ON tb_ev_2025, tb_ev_2026 TO regress_261_tview;
SET ROLE regress_261_tview;
SELECT pg_tviews_create('tv_ev', $$
    SELECT pk_ev, id, jsonb_build_object('day', day, 'amount', amount) AS data FROM tb_ev $$);
DO $$ BEGIN
    IF has_table_privilege('tb_ev_2025', 'SELECT') THEN
        RAISE EXCEPTION 'FAIL: setup: the TVIEW owner can read the partition';
    END IF;
END $$;
DO $$ BEGIN
    DROP TABLE tb_ev_2025;
    RAISE EXCEPTION 'FAIL: a role that does not own the partition dropped it';
EXCEPTION WHEN insufficient_privilege THEN NULL;
END $$;
SET ROLE regress_261_owner;
DROP TABLE tb_ev_2025;
RESET ROLE;
DO $$ BEGIN
    IF (SELECT count(*) FROM tv_ev) <> 1 OR EXISTS (SELECT 1 FROM tv_ev WHERE pk_ev = 1) THEN
        RAISE EXCEPTION 'FAIL: drop of a partition the TVIEW owner cannot read: tv_ev has % rows',
            (SELECT count(*) FROM tv_ev);
    END IF;
END $$;
-- Read directly, a partition applies its own row security: one that hides its
-- rows from the TVIEW owner is refreshed in full too.
SET ROLE regress_261_owner;
GRANT SELECT ON tb_ev_2026 TO regress_261_tview;
ALTER TABLE tb_ev_2026 ENABLE ROW LEVEL SECURITY;
DROP TABLE tb_ev_2026;
RESET ROLE;
DO $$ BEGIN
    IF (SELECT count(*) FROM tv_ev) <> 0 THEN
        RAISE EXCEPTION 'FAIL: drop of a partition whose row security hides its rows: tv_ev has % rows',
            (SELECT count(*) FROM tv_ev);
    END IF;
END $$;
-- DETACH is the partitioned table's owner's to run, whoever owns the partition.
CREATE TABLE tb_dt (pk_dt bigint NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(), PRIMARY KEY (pk_dt))
    PARTITION BY RANGE (pk_dt);
CREATE TABLE tb_dt_1 PARTITION OF tb_dt FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_dt_2 PARTITION OF tb_dt FOR VALUES FROM (100) TO (200);
INSERT INTO tb_dt (pk_dt) VALUES (1), (150);
ALTER TABLE tb_dt OWNER TO regress_261_owner;
ALTER TABLE tb_dt_1 OWNER TO regress_261_tview;
SELECT pg_tviews_create('tv_dt', $$ SELECT pk_dt, id, jsonb_build_object('pk', pk_dt) AS data FROM tb_dt $$);
SET ROLE regress_261_owner;
ALTER TABLE tb_dt DETACH PARTITION tb_dt_1;
RESET ROLE;
DO $$ BEGIN
    IF (SELECT array_agg(pk_dt) FROM tv_dt) IS DISTINCT FROM ARRAY[150::bigint] THEN
        RAISE EXCEPTION 'FAIL: detach by the partitioned table''s owner: tv_dt holds %',
            (SELECT array_agg(pk_dt) FROM tv_dt);
    END IF;
END $$;
DROP TABLE tv_dt;
DROP TABLE tb_dt, tb_dt_1;
DROP OWNED BY regress_261_owner, regress_261_tview CASCADE;
DROP ROLE regress_261_owner;
DROP ROLE regress_261_tview;

-- ── a TVIEW over a partitioned table below the root ────────────────────────
CREATE TABLE tb_m (pk_m bigint NOT NULL, grp int NOT NULL, PRIMARY KEY (pk_m, grp))
    PARTITION BY LIST (grp);
CREATE TABLE tb_m_1 PARTITION OF tb_m FOR VALUES IN (1) PARTITION BY RANGE (pk_m);
CREATE TABLE tb_m_1a PARTITION OF tb_m_1 FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_m_1b PARTITION OF tb_m_1 FOR VALUES FROM (100) TO (200);
INSERT INTO tb_m VALUES (1, 1), (150, 1);
SELECT pg_tviews_create('tv_m', $$
    SELECT pk_m, md5(pk_m::text)::uuid AS id, jsonb_build_object('grp', grp) AS data FROM tb_m_1 $$);
DROP TABLE tb_m_1a;
DO $$ BEGIN
    IF (SELECT array_agg(pk_m) FROM tv_m) IS DISTINCT FROM ARRAY[150::bigint] THEN
        RAISE EXCEPTION 'FAIL: drop below a TVIEW over a middle level: tv_m holds %',
            (SELECT array_agg(pk_m) FROM tv_m);
    END IF;
END $$;

-- ── a foreign partition ─────────────────────────────────────────────────────
-- It is not read (it may be unreachable): its rows leave with a full refresh.
CREATE EXTENSION IF NOT EXISTS file_fdw;
CREATE SERVER regress_261_files FOREIGN DATA WRAPPER file_fdw;
CREATE TABLE tb_f (pk_f bigint NOT NULL, day date NOT NULL) PARTITION BY RANGE (day);
CREATE TABLE tb_f_2026 PARTITION OF tb_f FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
CREATE FOREIGN TABLE tb_f_2025 PARTITION OF tb_f FOR VALUES FROM ('2025-01-01') TO ('2026-01-01')
    SERVER regress_261_files OPTIONS (program 'echo 7,2025-03-01', format 'csv');
INSERT INTO tb_f VALUES (8, '2026-03-01');
SELECT pg_tviews_create('tv_f', $$
    SELECT pk_f, md5(pk_f::text)::uuid AS id, jsonb_build_object('day', day) AS data FROM tb_f $$);
DROP FOREIGN TABLE tb_f_2025;
DO $$ BEGIN
    IF (SELECT array_agg(pk_f) FROM tv_f) IS DISTINCT FROM ARRAY[8::bigint] THEN
        RAISE EXCEPTION 'FAIL: drop of a foreign partition: tv_f holds %', (SELECT array_agg(pk_f) FROM tv_f);
    END IF;
END $$;
DROP TABLE tv_f;
DROP TABLE tb_f;
DROP SERVER regress_261_files;

\echo 'partition drop: PASS'
