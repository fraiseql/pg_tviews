-- Regression test (#134): CREATE TABLE tv_* AS SELECT runs the shared create code.
--
-- The ProcessUtility hook used to let PostgreSQL create a plain table, then drop it
-- from the event trigger, rebuild it as a TVIEW and populate it later. It now turns
-- the statement into a call of the code behind pg_tviews_create_or_replace(), with
-- CREATE TABLE AS semantics (an existing TVIEW is an error, IF NOT EXISTS makes it
-- a notice), before PostgreSQL creates anything. UNLOGGED and WITH (fillfactor)
-- are honoured; what the conversion cannot honour is refused with a hint, and so is
-- EXPLAIN of such a statement. The command tag reports the rows, as for any CTAS.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_134_ctas.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#134 CTAS FAIL: %', what; END IF; END $$;
-- The error and hint a statement raises, or NULL.
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE msg text; hint text;
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    RETURN msg || ' | ' || coalesce(hint, '');
END $$;
CREATE FUNCTION refused(stmt text, what text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE e text := error_of(stmt);
BEGIN
    PERFORM must(e LIKE '%pg_tviews_create_or_replace%', format('%s not refused with the hint: %s', what, e));
    PERFORM must(to_regclass('tv_x') IS NULL AND to_regclass('v_x') IS NULL,
                 format('%s left objects behind', what));
END $$;

CREATE TABLE tb_x (pk_x int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_x (pk_x, name) VALUES (1, 'a'), (2, 'b'), (3, 'c');

-- 1. The conversion is synchronous and reports its rows.
CREATE TABLE tv_x AS
    SELECT pk_x, id, jsonb_build_object('name', name) AS data FROM tb_x;
SELECT must(:ROW_COUNT = 3, 'command tag SELECT 3');
SELECT must((SELECT count(*) FROM tv_x) = 3, 'populated by the statement itself');
SELECT must(EXISTS (SELECT 1 FROM tviews.registry WHERE entity = 'x'), 'registered');
UPDATE tb_x SET name = 'a2' WHERE pk_x = 1;
SELECT must((SELECT data->>'name' FROM tv_x WHERE pk_x = 1) = 'a2', 'follows its base table');

-- 2. CREATE TABLE AS semantics on an existing TVIEW.
SELECT must(error_of($$CREATE TABLE tv_x AS
    SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x$$)
    LIKE '%already exists%pg_tviews_create_or_replace%', 'existing TVIEW is an error');
SET client_min_messages TO NOTICE;
CREATE TABLE IF NOT EXISTS tv_x AS
    SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x;
SET client_min_messages TO WARNING;
SELECT must((SELECT data ? 'name' FROM tv_x WHERE pk_x = 1), 'IF NOT EXISTS left the TVIEW alone');
SELECT tviews.pg_tviews_drop('x');

-- 3. UNLOGGED and WITH (fillfactor = n) are honoured.
CREATE UNLOGGED TABLE tv_x WITH (fillfactor = 70) AS
    SELECT pk_x, id, jsonb_build_object('name', name) AS data FROM tb_x;
SELECT must((SELECT NOT logged AND (options->>'fillfactor')::int = 70
             FROM tviews.registry WHERE entity = 'x'), 'UNLOGGED and fillfactor');
SELECT tviews.pg_tviews_drop('x');

-- 4. What the conversion cannot honour is refused.
SELECT refused($$SELECT pk_x, id, jsonb_build_object('name', name) AS data INTO tv_x FROM tb_x$$,
               'SELECT INTO');
SELECT refused($$CREATE TEMP TABLE tv_x AS SELECT pk_x, id, jsonb_build_object() AS data FROM tb_x$$,
               'TEMP');
SELECT refused($$CREATE TABLE tv_x (a, b, c) AS SELECT pk_x, id, jsonb_build_object() FROM tb_x$$,
               'column names');
SELECT refused($$CREATE TABLE tv_x TABLESPACE pg_default AS
                 SELECT pk_x, id, jsonb_build_object() AS data FROM tb_x$$, 'TABLESPACE');
SELECT refused($$CREATE TABLE tv_x USING heap AS
                 SELECT pk_x, id, jsonb_build_object() AS data FROM tb_x$$, 'USING');
SELECT refused($$CREATE TABLE tv_x WITH (autovacuum_enabled = false) AS
                 SELECT pk_x, id, jsonb_build_object() AS data FROM tb_x$$, 'other reloption');
SELECT refused($$CREATE TABLE tv_x AS
                 SELECT pk_x, id, jsonb_build_object() AS data FROM tb_x WITH NO DATA$$, 'WITH NO DATA');
PREPARE q AS SELECT pk_x, id, jsonb_build_object('name', name) AS data FROM tb_x;
SELECT refused($$CREATE TABLE tv_x AS EXECUTE q$$, 'AS EXECUTE');
DEALLOCATE q;
CREATE FUNCTION ctas_with_parameter(n text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('name', n) AS data FROM tb_x;
END $$;
SELECT refused($$SELECT ctas_with_parameter('p')$$, 'a query with parameters');
SELECT refused($$EXPLAIN CREATE TABLE tv_x AS
                 SELECT pk_x, id, jsonb_build_object() AS data FROM tb_x$$, 'EXPLAIN');
SELECT refused($$EXPLAIN ANALYZE CREATE TABLE tv_x AS
                 SELECT pk_x, id, jsonb_build_object() AS data FROM tb_x$$, 'EXPLAIN ANALYZE');

-- 5. A materialized view named tv_* is left to PostgreSQL.
CREATE MATERIALIZED VIEW tv_mat AS SELECT 1 AS n;
SELECT must((SELECT relkind FROM pg_class WHERE oid = 'tv_mat'::regclass) = 'm'
            AND NOT EXISTS (SELECT 1 FROM tviews.registry WHERE name = 'tv_mat'),
            'materialized view untouched');
DROP MATERIALIZED VIEW tv_mat;

-- 6. Anywhere: a DO block, an explicit transaction, a multi-statement batch.
DO $$ BEGIN
    CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('name', name) AS data FROM tb_x;
END $$;
SELECT must((SELECT count(*) FROM tv_x) = 3, 'DO block');
SELECT tviews.pg_tviews_drop('x');
BEGIN;
CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('name', name) AS data FROM tb_x;
UPDATE tb_x SET name = 'b2' WHERE pk_x = 2;
COMMIT;
SELECT must((SELECT data->>'name' FROM tv_x WHERE pk_x = 2) = 'b2', 'explicit transaction');
SELECT tviews.pg_tviews_drop('x');
CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('name', name) AS data FROM tb_x \; UPDATE tb_x SET name = 'c2' WHERE pk_x = 3;
SELECT must((SELECT data->>'name' FROM tv_x WHERE pk_x = 3) = 'c2', 'multi-statement batch');

-- 7. pg_tviews_create() is create-only on the same code: the name must match.
SELECT must(error_of($$SELECT tviews.pg_tviews_create('tv_x', 'SELECT 1')$$)
            LIKE '%already exists%', 'pg_tviews_create on an existing TVIEW');
SELECT must(error_of($$SELECT tviews.pg_tviews_create('tv_y',
    'SELECT pk_x, id, jsonb_build_object() AS data FROM tb_x')$$) LIKE '%pk_x%',
            'pg_tviews_create with a name that does not match the key');

DROP EXTENSION pg_tviews CASCADE;

SELECT 'issue #134 CTAS: PASS' AS result;
-- expect-output: issue #134 CTAS: PASS
