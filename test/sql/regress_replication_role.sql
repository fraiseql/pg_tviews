-- session_replication_role decides which pg_tviews triggers fire.
--
-- Under `replica`, triggers enabled for the origin (the default) do not fire: a
-- write is not seen, nothing is queued, and the TVIEW stays as it was until a
-- refresh (documented). Triggers enabled ALWAYS fire in both roles, and the
-- statement frame logic must treat them as firing: the write is refreshed at the
-- end of its statement. Triggers enabled REPLICA fire only under `replica`.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_replication_role.sql
-- expect-output: replication role: all fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n int);
INSERT INTO tb_item SELECT g, gen_random_uuid(), 0 FROM generate_series(1, 3) g;
SELECT pg_tviews_create('tv_item',
  $q$SELECT pk_item, id, jsonb_build_object('id', id, 'n', n) AS data FROM tb_item$q$);

-- Set how the TVIEW's triggers on tb_item fire: ALWAYS, REPLICA or ORIGIN.
CREATE FUNCTION tview_triggers_fire(how text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE t name;
BEGIN
    FOR t IN SELECT tgname FROM pg_trigger
             WHERE tgrelid = 'tb_item'::regclass AND tgname LIKE 'trg\_tview%' LOOP
        EXECUTE format('ALTER TABLE tb_item ENABLE %s TRIGGER %I',
                       CASE how WHEN 'origin' THEN '' ELSE how END, t);
    END LOOP;
END $$;

-- Default triggers under replica: the write is not seen.
SET session_replication_role = replica;
UPDATE tb_item SET n = 1;
RESET session_replication_role;
DO $$ BEGIN
    IF fresh_diff('tv_item', 'pk_item') IS NULL THEN
        RAISE EXCEPTION 'origin-enabled triggers fired under session_replication_role = replica';
    END IF;
END $$;
SELECT pg_tviews_refresh('item');
SELECT assert_fresh('tv_item', 'pk_item', 'pg_tviews_refresh after a replica write');

-- ALWAYS: fires under replica, in and out of a transaction block.
SELECT tview_triggers_fire('always');
SET session_replication_role = replica;
UPDATE tb_item SET n = 2;
SELECT assert_fresh('tv_item', 'pk_item', 'a replica write with ALWAYS triggers');
BEGIN;
UPDATE tb_item SET n = 3 WHERE pk_item = 1;
INSERT INTO tb_item (pk_item, n) VALUES (4, 4);
COMMIT;
SELECT assert_fresh('tv_item', 'pk_item', 'a replica transaction with ALWAYS triggers');
RESET session_replication_role;
UPDATE tb_item SET n = 5;
SELECT assert_fresh('tv_item', 'pk_item', 'an origin write with ALWAYS triggers');

-- REPLICA: fires only under replica.
SELECT tview_triggers_fire('replica');
SET session_replication_role = replica;
UPDATE tb_item SET n = 6;
SELECT assert_fresh('tv_item', 'pk_item', 'a replica write with REPLICA triggers');
RESET session_replication_role;
SELECT tview_triggers_fire('origin');
UPDATE tb_item SET n = 7;
SELECT assert_fresh('tv_item', 'pk_item', 'an origin write after restoring ORIGIN triggers');

\echo 'replication role: all fresh'
