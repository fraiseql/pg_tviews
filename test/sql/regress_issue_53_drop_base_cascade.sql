-- Regression test for issue #53:
--   "DROP TABLE tb_<entity> CASCADE orphans the tv_<entity> table and leaves
--    stale pg_tview_meta."
--
-- The base-table -> tview link is not a hard PostgreSQL dependency, so PG's
-- CASCADE removes the backing view v_<entity> and the base-table triggers
-- (both hard deps) but never the trigger-populated tv_<entity> table or its
-- pg_tview_meta row.
--
-- Correct behaviour: dropping the base table with CASCADE also deregisters and
-- drops the dependent tview, leaving no orphaned tv_* table and no stale
-- metadata. A direct DROP TABLE tv_* still works (no double-handling), and an
-- unrelated tb_* table with no tview drops cleanly.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_53_drop_base_cascade.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_widget CASCADE;
DROP VIEW  IF EXISTS v_widget CASCADE;
DROP TABLE IF EXISTS tb_widget CASCADE;
DROP TABLE IF EXISTS tv_gadget CASCADE;
DROP VIEW  IF EXISTS v_gadget CASCADE;
DROP TABLE IF EXISTS tb_gadget CASCADE;
DROP TABLE IF EXISTS tb_plain CASCADE;

-- ========================================================================
-- Cycle 1: DROP TABLE tb_<entity> CASCADE cleans up its tview + metadata
-- ========================================================================
CREATE TABLE tb_widget (
    pk_widget INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name      TEXT
);
INSERT INTO tb_widget (name) VALUES ('w1');
SELECT pg_tviews_create('tv_widget', $TVIEW$
    SELECT pk_widget, id, jsonb_build_object('name', name) AS data FROM tb_widget
$TVIEW$);

DO $$ BEGIN
  IF to_regclass('tv_widget') IS NULL THEN
    RAISE EXCEPTION '#53 setup FAIL: tv_widget did not materialize';
  END IF;
END $$;

DROP TABLE tb_widget CASCADE;

DO $$ BEGIN
  IF to_regclass('tv_widget') IS NOT NULL THEN
    RAISE EXCEPTION '#53 FAIL: tv_widget table orphaned after DROP TABLE tb_widget CASCADE';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'widget') THEN
    RAISE EXCEPTION '#53 FAIL: stale pg_tview_meta row for entity=widget after base drop';
  END IF;
  IF to_regclass('v_widget') IS NOT NULL THEN
    RAISE EXCEPTION '#53 FAIL: backing view v_widget still present after base drop';
  END IF;
END $$;

-- ========================================================================
-- Cycle 2a: a direct DROP TABLE tv_* still deregisters exactly once
-- (no double-handling error from the new sql_drop trigger)
-- ========================================================================
CREATE TABLE tb_gadget (
    pk_gadget INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name      TEXT
);
INSERT INTO tb_gadget (name) VALUES ('g1');
SELECT pg_tviews_create('tv_gadget', $TVIEW$
    SELECT pk_gadget, id, jsonb_build_object('name', name) AS data FROM tb_gadget
$TVIEW$);

-- Direct drop of the tview must succeed and clean metadata, base table untouched.
SELECT pg_tviews_drop('tv_gadget', true, true);
DO $$ BEGIN
  IF to_regclass('tv_gadget') IS NOT NULL THEN
    RAISE EXCEPTION '#53 FAIL: direct pg_tviews_drop left tv_gadget behind';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'gadget') THEN
    RAISE EXCEPTION '#53 FAIL: direct drop left stale metadata for gadget';
  END IF;
  IF to_regclass('tb_gadget') IS NULL THEN
    RAISE EXCEPTION '#53 FAIL: direct tview drop should not remove the base table tb_gadget';
  END IF;
END $$;
-- base table cleanup for this cycle
DROP TABLE tb_gadget CASCADE;

-- ========================================================================
-- Cycle 2b: an unrelated tb_* table with no tview drops cleanly
-- (the sql_drop handler must ignore base tables that back no entity)
-- ========================================================================
CREATE TABLE tb_plain (pk_plain INTEGER PRIMARY KEY, name TEXT);
DROP TABLE tb_plain CASCADE;   -- must not raise from the event trigger
DO $$ BEGIN
  IF to_regclass('tb_plain') IS NOT NULL THEN
    RAISE EXCEPTION '#53 FAIL: tb_plain not dropped';
  END IF;
END $$;

SELECT '#53 PASS: base-table CASCADE deregisters the tview; direct + unrelated drops unaffected' AS result;
