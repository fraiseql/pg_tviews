-- A TVIEW never reads, unannounced, a relation no trigger can watch:
--
--   * a foreign table (its rows change on another server) and a sequence
--     (nextval() changes it) are uncascaded reads, which the TVIEW's
--     uncascaded_policy decides: refused by default, accepted under "warn";
--     "full_refresh" is refused, since nothing would ever start the refresh;
--   * a table read with its inheritance children (no ONLY) is refused: writes to
--     a child fire no trigger on the parent. FROM ONLY is accepted, and a table a
--     TVIEW reads gets no new inheritance child.
--
-- Before 0.1.0-beta.34 all three were accepted under the default policy and the
-- TVIEW went stale.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_untrackable_sources.sql
--
-- expect-output: untrackable sources: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP EXTENSION IF EXISTS file_fdw CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE EXTENSION file_fdw;

-- 'SQLSTATE: message' of the error `stmt` raises, or NULL.
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ': ' || SQLERRM;
END $$;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF ok IS NOT TRUE THEN RAISE EXCEPTION 'untrackable sources FAIL: %', msg; END IF;
END $$;

CREATE TABLE tb_loc (pk_loc bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     fk_remote bigint);
INSERT INTO tb_loc VALUES (1, DEFAULT, 1);
CREATE SERVER regress_untrackable FOREIGN DATA WRAPPER file_fdw;
CREATE FOREIGN TABLE tb_remote (pk_remote bigint, name text)
    SERVER regress_untrackable OPTIONS (program 'echo 1,alpha', format 'csv');

-- ── a foreign table ──────────────────────────────────────────────────────────
\set foreign_def 'SELECT l.pk_loc, l.id, jsonb_build_object(''r'', (SELECT r.name FROM tb_remote r WHERE r.pk_remote = l.fk_remote)) AS data FROM tb_loc l'
SELECT must(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_loc', :'foreign_def'))
            LIKE '22023: writes to public.tb_remote would not refresh public.tv_loc (a foreign table:%',
            'the default policy refuses a foreign table: '
            || coalesce(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_loc', :'foreign_def')), 'created'));
SELECT must(error_of(format('SELECT tviews.pg_tviews_create(%L, %L, %L::jsonb)', 'tv_loc', :'foreign_def',
                            '{"uncascaded_policy": "full_refresh"}'))
            LIKE '22023: %the full_refresh policy would never run',
            'full_refresh cannot apply to a foreign table');
SELECT tviews.pg_tviews_create('tv_loc', :'foreign_def', '{"uncascaded_policy": "warn"}'::jsonb);
SELECT must((SELECT uncascaded_tables = '{tb_remote}' FROM tviews.registry WHERE entity = 'loc'),
            'tb_remote is listed uncascaded: '
            || (SELECT uncascaded_tables::text FROM tviews.registry WHERE entity = 'loc'));
SELECT must(NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'tb_remote'::regclass),
            'no trigger on the foreign table');
DROP TABLE tv_loc;

-- ── a sequence ───────────────────────────────────────────────────────────────
CREATE SEQUENCE s_counter;
\set sequence_def 'SELECT l.pk_loc AS pk_count, l.id, jsonb_build_object(''last'', (SELECT last_value FROM s_counter)) AS data FROM tb_loc l'
SELECT must(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_count', :'sequence_def'))
            LIKE '22023: writes to public.s_counter would not refresh public.tv_count (a sequence:%',
            'the default policy refuses a sequence: '
            || coalesce(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_count', :'sequence_def')), 'created'));
SELECT tviews.pg_tviews_create('tv_count', :'sequence_def', '{"uncascaded_policy": "warn"}'::jsonb);
SELECT must((SELECT uncascaded_tables = '{s_counter}' FROM tviews.registry WHERE entity = 'count'),
            's_counter is listed uncascaded');
DROP TABLE tv_count;

-- ── inheritance children ─────────────────────────────────────────────────────
CREATE TABLE tb_animal (pk_animal bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
CREATE TABLE tb_dog (bark text) INHERITS (tb_animal);
INSERT INTO tb_dog VALUES (2, gen_random_uuid(), 'rex', 'woof');
SELECT must(error_of($$SELECT tviews.pg_tviews_create('tv_animal', $d$
                SELECT pk_animal, id, jsonb_build_object('name', name) AS data FROM tb_animal $d$)$$)
            LIKE '0A000: %the definition reads public.tb_animal with its inheritance children '
                 '(public.tb_dog): writes to them fire no trigger on public.tb_animal, and the '
                 'TVIEW would go stale; read FROM ONLY public.tb_animal, or make '
                 'public.tb_animal a partitioned table',
            'a table read with its children is refused: '
            || coalesce(error_of($$SELECT tviews.pg_tviews_create('tv_animal', $d$
                SELECT pk_animal, id, jsonb_build_object('name', name) AS data FROM tb_animal $d$)$$), 'created'));
-- FROM ONLY reads the parent's own rows, which its triggers see.
SELECT tviews.pg_tviews_create('tv_animal', $$
    SELECT pk_animal, id, jsonb_build_object('name', name) AS data FROM ONLY tb_animal $$);
INSERT INTO tb_animal VALUES (1, gen_random_uuid(), 'cat');
INSERT INTO tb_dog VALUES (3, gen_random_uuid(), 'fido', 'arf');
SELECT must((SELECT array_agg(pk_animal ORDER BY pk_animal) = '{1}' FROM tv_animal),
            'tv_animal holds the parent''s own rows only');
-- A new child of a table a TVIEW reads is refused, by CREATE TABLE and by ALTER
-- TABLE: a TVIEW reading it with its children would go stale.
SELECT must(error_of('CREATE TABLE tb_cat (purr text) INHERITS (tb_animal)')
            LIKE '0A000: %tb_cat cannot inherit from public.tb_animal: tv_animal reads it%',
            'CREATE TABLE … INHERITS a TVIEW''s table: '
            || coalesce(error_of('CREATE TABLE tb_cat (purr text) INHERITS (tb_animal)'), 'created'));
SELECT must(error_of('CREATE FOREIGN TABLE tb_fox () INHERITS (tb_animal) SERVER regress_untrackable '
                     || 'OPTIONS (program ''echo 1'', format ''csv'')')
            LIKE '0A000: %tb_fox cannot inherit from public.tb_animal: tv_animal reads it%',
            'CREATE FOREIGN TABLE … INHERITS a TVIEW''s table: '
            || coalesce(error_of('CREATE FOREIGN TABLE tb_fox () INHERITS (tb_animal) SERVER regress_untrackable '
                                 || 'OPTIONS (program ''echo 1'', format ''csv'')'), 'created'));
CREATE TABLE tb_bird (pk_animal bigint, id uuid, name text);
SELECT must(error_of('ALTER TABLE tb_bird INHERIT tb_animal')
            LIKE '0A000: %tb_bird cannot inherit from public.tb_animal: tv_animal reads it%',
            'ALTER TABLE … INHERIT a TVIEW''s table');
-- Also a table a TVIEW joins (statement triggers only, no row trigger).
CREATE TABLE tb_kind (pk_kind bigint PRIMARY KEY, label text);
CREATE TABLE tb_pet (pk_pet bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     fk_kind bigint);
SELECT tviews.pg_tviews_create('tv_pet', $$
    SELECT p.pk_pet, p.id, jsonb_build_object('kind', k.label) AS data
    FROM tb_pet p JOIN tb_kind k ON k.pk_kind = p.fk_kind $$);
SELECT must(error_of('CREATE TABLE tb_kind_old () INHERITS (tb_kind)')
            LIKE '0A000: %tb_kind_old cannot inherit from public.tb_kind: tv_pet reads it%',
            'CREATE TABLE … INHERITS a table a TVIEW joins: '
            || coalesce(error_of('CREATE TABLE tb_kind_old () INHERITS (tb_kind)'), 'created'));
-- A table no TVIEW reads inherits as before.
CREATE TABLE tb_free (x int);
CREATE TABLE tb_free_child () INHERITS (tb_free);

\echo 'untrackable sources: PASS'
