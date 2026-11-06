-- Every way a TVIEW goes frees its tviews.stats slot: pg_tviews_drop, DROP
-- SCHEMA … CASCADE (by a superuser, or by the schema's owner over another role's
-- TVIEW), DROP OWNED, and a drop in the transaction that wrote the TVIEW; and a
-- TVIEW created in a transaction or subtransaction that rolls back takes none.
-- The slots are a fixed table shared by the cluster: one kept by a TVIEW that is
-- gone is lost until a restart. Test builds list the slots each database holds
-- in pg_tviews_show_queue().
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_drop_stats_slots.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

SELECT tviews.pg_tviews_show_queue() ? 'stats_slots_by_database' AS hooks \gset
\if :hooks
\else
\echo 'SKIP: stats_slots_by_database needs a test-hooks build'
\quit
\endif

\ir ../../lib/who.sql
SELECT regress_roles('regress_dss');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'drop stats slots FAIL: %', what; END IF; END $$;
CREATE FUNCTION slots() RETURNS int LANGUAGE sql AS $$
    SELECT coalesce((tviews.pg_tviews_show_queue()->'stats_slots_by_database'
                     ->>(SELECT oid FROM pg_database WHERE datname = current_database())::text)::int, 0) $$;
GRANT EXECUTE ON FUNCTION must(boolean, text), slots() TO PUBLIC;
-- Create s.tv_<entity> over s.tb_<entity> and write one row: one slot.
CREATE FUNCTION written(s text, entity text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE format('CREATE TABLE %1$I.tb_%2$s (pk_%2$s int PRIMARY KEY, '
                   'id uuid NOT NULL DEFAULT gen_random_uuid(), v text)', s, entity);
    PERFORM tviews.pg_tviews_create(format('%I.tv_%s', s, entity),
        format('SELECT pk_%2$s, id, jsonb_build_object(''v'', v) AS data FROM %1$I.tb_%2$s', s, entity));
    EXECUTE format('INSERT INTO %I.tb_%s VALUES (1, DEFAULT, ''x'')', s, entity);
END $$;
GRANT EXECUTE ON FUNCTION written(text, text) TO PUBLIC;

SELECT slots() AS base \gset

-- pg_tviews_drop.
CREATE SCHEMA dss_a;
SELECT written('dss_a', 'one');
SELECT must(slots() = :base + 1, format('a written TVIEW holds no slot: %s', slots()));
SELECT tviews.pg_tviews_drop('dss_a.tv_one');
SELECT must(slots() = :base, format('pg_tviews_drop kept the slot: %s', slots()));

-- DROP SCHEMA … CASCADE by a superuser.
SELECT written('dss_a', 'two');
DROP SCHEMA dss_a CASCADE;
SELECT must(slots() = :base, format('DROP SCHEMA CASCADE kept the slot: %s', slots()));

-- DROP SCHEMA … CASCADE by the schema's owner, over another role's TVIEW.
CREATE SCHEMA dss_b AUTHORIZATION regress_dss_owner_a;
GRANT USAGE, CREATE ON SCHEMA dss_b TO regress_dss_owner_b;
SET ROLE regress_dss_owner_b;
SELECT written('dss_b', 'three');
RESET ROLE;
SELECT must(slots() = :base + 1, format('another role''s written TVIEW holds no slot: %s', slots()));
SET ROLE regress_dss_owner_a;
DROP SCHEMA dss_b CASCADE;
RESET ROLE;
SELECT must(slots() = :base, format('the schema owner''s DROP SCHEMA CASCADE kept the slot: %s', slots()));

-- DROP OWNED.
CREATE SCHEMA dss_c;
GRANT USAGE, CREATE ON SCHEMA dss_c TO regress_dss_owner_b;
SET ROLE regress_dss_owner_b;
SELECT written('dss_c', 'four');
RESET ROLE;
DROP OWNED BY regress_dss_owner_b;
SELECT must(slots() = :base, format('DROP OWNED kept the slot: %s', slots()));

-- A drop in the transaction that wrote the TVIEW.
SELECT written('dss_c', 'five');
BEGIN;
INSERT INTO dss_c.tb_five VALUES (2, DEFAULT, 'y');
SELECT tviews.pg_tviews_drop('dss_c.tv_five');
COMMIT;
SELECT must(slots() = :base, format('a drop after a write in its transaction kept the slot: %s', slots()));

-- Created and written in a transaction that rolls back.
BEGIN;
SELECT written('dss_c', 'six');
ROLLBACK;
SELECT must(slots() = :base, format('a rolled-back create took a slot: %s', slots()));
-- In a subtransaction that rolls back, the transaction committing.
BEGIN;
SAVEPOINT s;
SELECT written('dss_c', 'seven');
ROLLBACK TO SAVEPOINT s;
COMMIT;
SELECT must(slots() = :base, format('a rolled-back savepoint''s create took a slot: %s', slots()));
-- Committed: one slot.
SELECT written('dss_c', 'eight');
SELECT must(slots() = :base + 1, format('a committed create took no slot: %s', slots()));
SELECT tviews.pg_tviews_drop('dss_c.tv_eight');

DROP SCHEMA dss_c CASCADE;
DROP EXTENSION pg_tviews CASCADE;
DROP FUNCTION must(boolean, text), slots(), written(text, text);
SELECT regress_drop_roles('regress_dss');
\echo 'drop stats slots: PASS'
