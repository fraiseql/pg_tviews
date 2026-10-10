-- Regression test for issue #210:
--   "pg_tviews_flush_and_report(reset) in a rolled-back savepoint loses the journal
--    entries it reported."
--
-- A report with reset (the default) drops the entries it reported, so the next one
-- lists only later changes. When the subtransaction that reported rolls back, the
-- rows those entries describe are still changed by the transaction, but the drop
-- stayed: a later report left them out.
--
-- Correct behaviour: rolling back a subtransaction undoes the resets it made, by a
-- savepoint or by a plpgsql EXCEPTION block. A committed subtransaction (RELEASE)
-- keeps its reset.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_flush_and_report_subxact.sql
-- expect-output: flush_and_report subxact: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'flush_and_report subxact FAIL: %', what; END IF; END $$;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'bob'), (3, 'cy');
-- The pk_user values a report lists as updated, sorted.
CREATE FUNCTION updated_users(report jsonb) RETURNS bigint[] LANGUAGE sql AS $$
    SELECT coalesce(array_agg(u.pk_user ORDER BY u.pk_user), '{}')
    FROM jsonb_array_elements(report->'updated') e JOIN tb_user u ON u.id = (e->>'id')::uuid $$;
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);

-- 1. A savepoint rolled back after a report: the next report lists the row again.
BEGIN;
UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
SAVEPOINT s;
SELECT must(updated_users(pg_tviews_flush_and_report(include_data => false)) = '{1}', 'first report');
ROLLBACK TO SAVEPOINT s;
SELECT must(updated_users(pg_tviews_flush_and_report(include_data => false)) = '{1}',
            'report after ROLLBACK TO SAVEPOINT lost user 1');
-- That report was outside any subtransaction: its reset holds.
SELECT must(updated_users(pg_tviews_flush_and_report(include_data => false)) = '{}', 'top-level reset');
COMMIT;

-- 2. A released savepoint keeps its reset.
BEGIN;
UPDATE tb_user SET name = 'h' WHERE pk_user = 2;
SAVEPOINT s;
SELECT must(updated_users(pg_tviews_flush_and_report(include_data => false)) = '{2}', 'report in released savepoint');
RELEASE SAVEPOINT s;
SELECT must(updated_users(pg_tviews_flush_and_report(include_data => false)) = '{}', 'released reset undone');
COMMIT;

-- 3. The mutation-function shape: report inside an EXCEPTION block that then
-- fails, report again from the handler. Writes inside the block are rolled back
-- and stay unreported; the write before it is reported.
CREATE FUNCTION mutate() RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE r jsonb;
BEGIN
    UPDATE tb_user SET name = 'outer' WHERE pk_user = 1;
    BEGIN
        UPDATE tb_user SET name = 'inner' WHERE pk_user = 3;
        r := pg_tviews_flush_and_report(include_data => false);
        PERFORM must(updated_users(r) = '{1,3}', 'report inside the block');
        RAISE EXCEPTION 'fail after report';
    EXCEPTION WHEN raise_exception THEN
        RETURN pg_tviews_flush_and_report(include_data => false);
    END;
END $$;
SELECT must(updated_users(mutate()) = '{1}', 'handler report: expected user 1 only');
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 3) = 'cy', 'rolled-back write left in tv_user');

-- 4. Nested: a reset in an inner savepoint that commits into an outer one that
-- rolls back is undone with the outer one.
BEGIN;
UPDATE tb_user SET name = 'n1' WHERE pk_user = 2;
SAVEPOINT a;
SAVEPOINT b;
SELECT must(updated_users(pg_tviews_flush_and_report(include_data => false)) = '{2}', 'nested first report');
RELEASE SAVEPOINT b;
SELECT must(updated_users(pg_tviews_flush_and_report(include_data => false)) = '{}', 'nested released reset');
ROLLBACK TO SAVEPOINT a;
SELECT must(updated_users(pg_tviews_flush_and_report(include_data => false)) = '{2}',
            'reset of a released inner savepoint survived the outer rollback');
COMMIT;

\echo 'flush_and_report subxact: PASS'
