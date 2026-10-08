-- A write whose key the plan cannot read fails loudly (Q26).
--
-- The column a local path reads a TVIEW's key from is read by the backing view,
-- so PostgreSQL refuses to drop it. If the plan names a column the table no
-- longer has anyway (a catalog edited by hand, a restore out of step), a write
-- to the table raises an ERROR naming the TVIEW, with the hint to re-register
-- it: the TVIEW is never left silently stale behind a log line.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_plan_key_column_gone.sql
-- expect-output: key column gone: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$$);

-- The view protects the column.
DO $$
BEGIN
    ALTER TABLE tb_user DROP COLUMN pk_user;
    RAISE EXCEPTION 'FAIL: the key column of a TVIEW was dropped';
EXCEPTION WHEN dependent_objects_still_exist THEN
    NULL;
END $$;

-- A plan naming a column the table does not have.
UPDATE tviews.pg_tview_meta
   SET plan = jsonb_set(plan, '{paths,0,initial_col}', '"pk_gone"')
 WHERE entity = 'user';

DO $$
DECLARE
    msg text;
    hint text;
BEGIN
    UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
    RAISE EXCEPTION 'FAIL: a write whose key cannot be read succeeded';
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    IF msg LIKE 'FAIL:%' THEN
        RAISE;
    END IF;
    IF msg NOT LIKE '%tv_user%' OR msg NOT LIKE '%pk_gone%'
       OR hint NOT LIKE '%pg_tviews_reregister%' THEN
        RAISE EXCEPTION 'FAIL: the error does not name the TVIEW, the column and the fix: % (hint: %)',
            msg, hint;
    END IF;
END $$;

-- Re-registering derives the plan again: writes refresh the TVIEW.
SELECT tviews.pg_tviews_reregister('user');
UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
DO $$
BEGIN
    IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) <> 'grace' THEN
        RAISE EXCEPTION 'FAIL: tv_user stale after re-registration';
    END IF;
END $$;

\echo 'key column gone: PASS'
