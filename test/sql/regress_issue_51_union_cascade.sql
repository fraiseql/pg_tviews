-- Regression test for issue #51 (part 1/3):
--   "Cross-table UNION branch changes do not cascade to the tview."
--
-- A UNION [ALL] tview whose branches read DIFFERENT base tables installs a
-- trigger on every branch table (pg_depend flattens the view), but
-- `extract_join_paths` errors on `SetExpr::SetOperation` and yields zero
-- cascade paths. The branch table that is NOT `tb_<entity>` therefore has a
-- firing trigger with no resolvable path, so its changes are dropped. Only the
-- branch that happens to be `tb_<entity>` refreshes (via the direct path).
--
-- Correct behaviour: a change in ANY branch's base table refreshes the tview
-- row for that key.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_51_union_cascade.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_task CASCADE;
DROP VIEW  IF EXISTS v_task CASCADE;
DROP TABLE IF EXISTS tb_task CASCADE;
DROP TABLE IF EXISTS tb_task_archive CASCADE;

-- Entity `task` unifies live tasks (tb_task = tb_<entity>, direct path) with
-- archived tasks (tb_task_archive, indirect — needs a cascade path). pk_task
-- values are disjoint across the two tables so each key lives in one branch.
CREATE TABLE tb_task (
    pk_task INTEGER PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL,
    title   TEXT
);
CREATE TABLE tb_task_archive (
    pk_task INTEGER PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL,
    title   TEXT
);
INSERT INTO tb_task (pk_task, title) VALUES (1, 'active-one'), (2, 'active-two');
INSERT INTO tb_task_archive (pk_task, title) VALUES (3, 'archived-three');

SELECT pg_tviews_create('tv_task', $TV$
  SELECT pk_task, id, jsonb_build_object('title', title, 'archived', false) AS data FROM tb_task
  UNION ALL
  SELECT pk_task, id, jsonb_build_object('title', title, 'archived', true) AS data FROM tb_task_archive
$TV$);

-- Baseline: both branches materialized.
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_task) <> 3 THEN
    RAISE EXCEPTION '#51 setup FAIL: expected 3 tview rows, got %', (SELECT count(*) FROM tv_task);
  END IF;
  IF (SELECT data->>'title' FROM tv_task WHERE pk_task = 3) <> 'archived-three' THEN
    RAISE EXCEPTION '#51 setup FAIL: archive branch not materialized';
  END IF;
END $$;

-- (1) Control: change in the direct branch (tb_task) must refresh.
UPDATE tb_task SET title = 'active-one-EDITED' WHERE pk_task = 1;
DO $$ BEGIN
  IF (SELECT data->>'title' FROM tv_task WHERE pk_task = 1) <> 'active-one-EDITED' THEN
    RAISE EXCEPTION '#51 FAIL: direct-branch change lost — title is %',
      (SELECT data->>'title' FROM tv_task WHERE pk_task = 1);
  END IF;
END $$;

-- (2) The gap: change in the indirect branch (tb_task_archive) must refresh.
UPDATE tb_task_archive SET title = 'archived-three-EDITED' WHERE pk_task = 3;
DO $$ BEGIN
  IF (SELECT data->>'title' FROM tv_task WHERE pk_task = 3) <> 'archived-three-EDITED' THEN
    RAISE EXCEPTION '#51 FAIL: UNION branch change lost — title is % (expected archived-three-EDITED)',
      (SELECT data->>'title' FROM tv_task WHERE pk_task = 3);
  END IF;
END $$;

-- (3) INSERT into the indirect branch must surface a new row.
INSERT INTO tb_task_archive (pk_task, title) VALUES (4, 'archived-four');
DO $$ BEGIN
  IF (SELECT data->>'title' FROM tv_task WHERE pk_task = 4) <> 'archived-four' THEN
    RAISE EXCEPTION '#51 FAIL: INSERT into UNION branch not reflected';
  END IF;
END $$;

-- (4) DELETE from the indirect branch must remove the row.
DELETE FROM tb_task_archive WHERE pk_task = 3;
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM tv_task WHERE pk_task = 3) THEN
    RAISE EXCEPTION '#51 FAIL: DELETE from UNION branch not propagated';
  END IF;
END $$;

SELECT '#51 PASS: cross-table UNION branch changes cascade to the tview' AS result;
