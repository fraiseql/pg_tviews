-- A TVIEW whose stored plan does not decode can still be dropped, cleanly.
--
-- DROP TABLE tv_* and pg_tviews_drop() need only the TVIEW's relations, not its
-- plan: they remove the catalog row, the backing view and the base-table
-- triggers. The DROP once took the table for a plain one when its plan did not
-- decode, and left the rest behind, so every write to another TVIEW's base table
-- kept failing.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_drop_with_unreadable_plan.sql
-- expect-output: drop_with_unreadable_plan: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_z (pk_z bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v text);
CREATE TABLE tb_w (pk_w bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
INSERT INTO tb_z (pk_z, v) VALUES (1, 'z');
INSERT INTO tb_w (pk_w, v) VALUES (1, 'w');
SELECT pg_tviews_create('tv_user', $$ SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT pg_tviews_create('tv_z', $$ SELECT pk_z, id, jsonb_build_object('v', v) AS data FROM tb_z $$);
SELECT pg_tviews_create('tv_w', $$ SELECT pk_w, id, jsonb_build_object('v', v) AS data FROM tb_w $$);

CREATE FUNCTION check_gone(entity text, label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tviews.pg_tview_meta m WHERE m.entity = check_gone.entity)
       OR to_regclass('tviews.public__tv_' || entity) IS NOT NULL
       OR EXISTS (SELECT 1 FROM pg_trigger WHERE tgname LIKE 'trg_tview_%_' || entity || '_on_%') THEN
        RAISE EXCEPTION 'FAIL (%): tv_% left its catalog row, backing view or triggers', label, entity;
    END IF;
END $$;

UPDATE tviews.pg_tview_meta SET plan = jsonb_set(plan, '{paths}', '"garbage"') WHERE entity IN ('z', 'w');

DROP TABLE tv_z;
SELECT check_gone('z', 'DROP TABLE');
SELECT pg_tviews_drop('tv_w');
SELECT check_gone('w', 'pg_tviews_drop');

-- The other TVIEW works again.
UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
DO $$ BEGIN
    IF (SELECT data->>'name' FROM tv_user) <> 'grace' THEN
        RAISE EXCEPTION 'FAIL: tv_user stale';
    END IF;
END $$;

\echo 'drop_with_unreadable_plan: PASS'
