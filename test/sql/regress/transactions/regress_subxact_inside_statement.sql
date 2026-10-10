-- A subtransaction that commits inside a writing statement keeps the refreshes
-- queued before it.
--
-- plpgsql runs a BEGIN … EXCEPTION … END block in a subtransaction. When such a
-- block runs inside a writing statement (an AFTER ROW audit trigger, a function in
-- the SET list), the refreshes and direct patches queued before the block started
-- must survive the block's commit. Each shape below writes several rows and then
-- checks every TVIEW against its backing view.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_subxact_inside_statement.sql
-- expect-output: subxact inside statement: all fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- ========================================================================
-- (a) AFTER ROW audit trigger with an EXCEPTION block, multi-row UPDATE
-- ========================================================================
CREATE TABLE tb_x (pk_x int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n text);
INSERT INTO tb_x SELECT g, gen_random_uuid(), 'a' FROM generate_series(1, 3) g;
SELECT pg_tviews_create('tv_x',
  $q$SELECT pk_x, id, jsonb_build_object('id', id, 'n', n) AS data FROM tb_x$q$);

CREATE TABLE audit_log (pk_x int, at timestamptz DEFAULT now());
CREATE FUNCTION audit_x() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    BEGIN
        INSERT INTO audit_log (pk_x) VALUES (NEW.pk_x);
    EXCEPTION WHEN others THEN
        NULL;
    END;
    RETURN NULL;
END $$;
CREATE TRIGGER audit_x AFTER UPDATE ON tb_x FOR EACH ROW EXECUTE FUNCTION audit_x();

UPDATE tb_x SET n = 'audited';
SELECT assert_fresh('tv_x', 'pk_x', 'a multi-row UPDATE under an EXCEPTION-block audit trigger');

-- The same inside an explicit transaction, with a savepoint around the write.
BEGIN;
SAVEPOINT s1;
UPDATE tb_x SET n = 'audited in a savepoint';
RELEASE SAVEPOINT s1;
COMMIT;
SELECT assert_fresh('tv_x', 'pk_x', 'the audited UPDATE inside a released savepoint');

-- ========================================================================
-- (b) UPDATE … SET x = f(x), where f runs an EXCEPTION block
-- ========================================================================
CREATE TABLE tb_y (pk_y int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n int);
INSERT INTO tb_y SELECT g, gen_random_uuid(), g FROM generate_series(1, 4) g;
SELECT pg_tviews_create('tv_y',
  $q$SELECT pk_y, id, jsonb_build_object('id', id, 'n', n) AS data FROM tb_y$q$);

CREATE FUNCTION safe_double(v int) RETURNS int LANGUAGE plpgsql AS $$
BEGIN
    BEGIN
        RETURN v * 2;
    EXCEPTION WHEN numeric_value_out_of_range THEN
        RETURN v;
    END;
END $$;

UPDATE tb_y SET n = safe_double(n);
SELECT assert_fresh('tv_y', 'pk_y', 'UPDATE … SET n = f(n) with an EXCEPTION block in f');

-- ========================================================================
-- (c) direct patch: a field captured inside the subtransaction joins the
--     fields captured before it
-- ========================================================================
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, bio text);
INSERT INTO tb_user SELECT g, gen_random_uuid(), 'u' || g, 'b' || g FROM generate_series(1, 3) g;
SELECT pg_tviews_create('tv_user',
  $q$SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user$q$);

-- On a rename, stamp the bio of the same row, inside an EXCEPTION block.
CREATE FUNCTION stamp_bio() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() = 1 THEN
        BEGIN
            UPDATE tb_user SET bio = bio || '!' WHERE pk_user = NEW.pk_user;
        EXCEPTION WHEN others THEN
            NULL;
        END;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER stamp_bio AFTER UPDATE OF name ON tb_user
    FOR EACH ROW EXECUTE FUNCTION stamp_bio();

UPDATE tb_user SET name = name || '-renamed';
SELECT assert_fresh('tv_user', 'pk_user', 'a rename whose trigger stamps bio inside an EXCEPTION block');

\echo 'subxact inside statement: all fresh'
