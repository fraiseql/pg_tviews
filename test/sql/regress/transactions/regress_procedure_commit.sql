-- A procedure that commits and rolls back leaves its TVIEWs fresh.
--
-- COMMIT and ROLLBACK inside a procedure end the transaction through SPI, not
-- through a top-level COMMIT statement, so pg_tviews' COMMIT intercept never sees
-- them. Every write of the procedure must still be refreshed before its
-- transaction commits, and a rolled-back write must leave nothing queued.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_procedure_commit.sql
-- expect-output: procedure commit: all fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n int);
INSERT INTO tb_item SELECT g, gen_random_uuid(), 0 FROM generate_series(1, 3) g;
SELECT pg_tviews_create('tv_item',
  $q$SELECT pk_item, id, jsonb_build_object('id', id, 'n', n) AS data FROM tb_item$q$);

CREATE PROCEDURE bump_in_steps() LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_item SET n = n + 1;
    COMMIT;
    INSERT INTO tb_item (pk_item, n) VALUES (4, 40);
    ROLLBACK;
    UPDATE tb_item SET n = n + 10 WHERE pk_item = 1;
    INSERT INTO tb_item (pk_item, n) VALUES (5, 50);
    COMMIT;
    UPDATE tb_item SET n = n + 100 WHERE pk_item = 2;
END $$;

CALL bump_in_steps();
SELECT assert_fresh('tv_item', 'pk_item', 'CALL of a procedure that commits and rolls back');

DELETE FROM tb_item WHERE pk_item >= 4;

-- A DO block that commits and rolls back.
DO $$
BEGIN
    UPDATE tb_item SET n = -n;
    COMMIT;
    UPDATE tb_item SET n = n - 1 WHERE pk_item = 3;
    ROLLBACK;
END $$;
SELECT assert_fresh('tv_item', 'pk_item', 'a DO block that commits and rolls back');

\echo 'procedure commit: all fresh'
