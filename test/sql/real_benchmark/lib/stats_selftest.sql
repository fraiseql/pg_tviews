-- Self-check for lib/stats.sql: a snapshot pair around 100 single-row UPDATEs
-- must report exactly 100 updated tuples, some WAL, and populated sizes.
\set ON_ERROR_STOP 1
\ir stats.sql

CREATE TABLE selftest (pk int PRIMARY KEY, v int NOT NULL);
INSERT INTO selftest SELECT g, 0 FROM generate_series(1, 100) g;

SELECT bench.reset();
SELECT pg_stat_force_next_flush();
SELECT bench.snapshot('before');
UPDATE selftest SET v = v + 1;
SELECT pg_stat_force_next_flush();
SELECT bench.snapshot('after', 100);

DO $$
DECLARE r record;
BEGIN
    SELECT * INTO STRICT r FROM bench.delta('before', 'after') WHERE relname = 'selftest';
    ASSERT r.n_tup_upd = 100, format('n_tup_upd = %s, expected 100', r.n_tup_upd);
    ASSERT r.n_tup_hot_upd BETWEEN 0 AND 100, 'n_tup_hot_upd out of range';
    ASSERT r.wal_bytes > 0, format('wal_bytes = %s, expected > 0', r.wal_bytes);
    ASSERT r.heap_bytes_after > 0, 'heap_bytes_after not populated';
    ASSERT r.index_bytes_after > 0, 'index_bytes_after not populated';
    ASSERT r.ops = 100, 'ops not carried from the closing snapshot';
    ASSERT (SELECT count(*) FROM bench.physical WHERE step = 'after' AND relname = 'selftest') = 1,
        'bench.physical has no row for the step';
END $$;

\echo 'STATS_SELFTEST_OK'
