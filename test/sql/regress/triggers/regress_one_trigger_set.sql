-- A table carries one set of pg_tviews triggers, however many TVIEWs read it:
-- `trg_tview_<tag>_on_<schema>_<table>`, no argument, the union of what its
-- readers need. Creating a second reader adds nothing; dropping a reader removes
-- nothing until the last one goes. A table only another TVIEW's delta mapping
-- reads (another TVIEW's table) gets the delta triggers alone. Each member of a
-- partition tree gets one flush and one truncate trigger. Every write still
-- refreshes each reader, and only the readers of what it changed.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/triggers/regress_one_trigger_set.sql
--
-- expect-output: one trigger set: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'one trigger set FAIL: %', what; END IF; END $$;

-- The pg_tviews triggers on `t` itself (not the copies PostgreSQL makes of a
-- partitioned table's row trigger), by name.
CREATE FUNCTION ours(t regclass) RETURNS text LANGUAGE sql AS $$
    SELECT coalesce(string_agg(tg.tgname, ',' ORDER BY tg.tgname), '')
    FROM pg_trigger tg JOIN pg_proc p ON p.oid = tg.tgfoid
    WHERE tg.tgrelid = t AND tg.tgparentid = 0
      AND p.pronamespace = 'tviews'::regnamespace $$;

CREATE TABLE tb_sku (pk_sku bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text, cost int NOT NULL DEFAULT 0, bio text);
CREATE TABLE tb_line (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_sku bigint, pos int);
CREATE TABLE tb_shelf (pk_shelf bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_sku bigint, label text);
INSERT INTO tb_sku (pk_sku, name) SELECT g, 's' || g FROM generate_series(1, 3) g;
INSERT INTO tb_line (pk_line, fk_sku, pos) SELECT g, 1 + g % 3, g FROM generate_series(1, 6) g;
INSERT INTO tb_shelf (pk_shelf, fk_sku, label) SELECT g, g, 'l' || g FROM generate_series(1, 3) g;

-- tb_sku: two row readers (sku, sku_cost) and one delta reader (line).
SELECT pg_tviews_create('tv_sku', $$
    SELECT s.pk_sku, s.id, jsonb_build_object('name', s.name, 'bio', s.bio) AS data
    FROM tb_sku s $$);
SELECT must(ours('tb_sku') = 'trg_tview_flush_on_public_tb_sku,trg_tview_row_on_public_tb_sku,'
                             'trg_tview_truncate_on_public_tb_sku',
            'one row reader: ' || ours('tb_sku'));
SELECT pg_tviews_create('tv_sku_cost', $$
    SELECT s.pk_sku AS pk_sku_cost, s.id, jsonb_build_object('cost', s.cost) AS data
    FROM tb_sku s $$);
SELECT must(ours('tb_sku') = 'trg_tview_flush_on_public_tb_sku,trg_tview_row_on_public_tb_sku,'
                             'trg_tview_truncate_on_public_tb_sku',
            'a second row reader adds nothing: ' || ours('tb_sku'));
SELECT pg_tviews_create('tv_line', $$
    SELECT l.pk_line, l.id, jsonb_build_object('pos', l.pos, 'sku', s.name) AS data
    FROM tb_line l JOIN tb_sku s ON s.pk_sku = l.fk_sku $$);
SELECT must(ours('tb_sku') = 'trg_tview_delta_d_on_public_tb_sku,trg_tview_delta_i_on_public_tb_sku,'
                             'trg_tview_delta_u_on_public_tb_sku,trg_tview_flush_on_public_tb_sku,'
                             'trg_tview_row_on_public_tb_sku,trg_tview_truncate_on_public_tb_sku',
            'a delta reader adds the delta triggers: ' || ours('tb_sku'));
-- tv_sku: read by shelf through its delta mapping only.
SELECT pg_tviews_create('tv_shelf', $$
    SELECT h.pk_shelf, h.id, jsonb_build_object('label', h.label, 'sku', t.data) AS data
    FROM tb_shelf h JOIN tv_sku t ON t.pk_sku = h.fk_sku $$);
SELECT must(ours('tv_sku') = 'trg_tview_delta_d_on_public_tv_sku,trg_tview_delta_i_on_public_tv_sku,'
                             'trg_tview_delta_u_on_public_tv_sku',
            'another TVIEW''s table gets the delta triggers alone: ' || ours('tv_sku'));
SELECT must(NOT EXISTS (SELECT 1 FROM pg_trigger tg JOIN pg_proc p ON p.oid = tg.tgfoid
                        WHERE p.pronamespace = 'tviews'::regnamespace AND tg.tgnargs <> 0),
            'a pg_tviews trigger carries an argument');

-- Every write refreshes each reader.
CREATE FUNCTION all_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('tv_sku', 'pk_sku', label);
    PERFORM assert_fresh('tv_sku_cost', 'pk_sku_cost', label);
    PERFORM assert_fresh('tv_line', 'pk_line', label);
    PERFORM assert_fresh('tv_shelf', 'pk_shelf', label);
END $$;
UPDATE tb_sku SET name = 'S2' WHERE pk_sku = 2;
SELECT all_fresh('UPDATE tb_sku.name');
UPDATE tb_sku SET cost = 7;
SELECT all_fresh('UPDATE tb_sku.cost');
INSERT INTO tb_sku (pk_sku, name) VALUES (4, 's4');
UPDATE tb_line SET fk_sku = 4 WHERE pk_line = 1;
SELECT all_fresh('INSERT tb_sku');
DELETE FROM tb_line WHERE fk_sku = 4;
DELETE FROM tb_sku WHERE pk_sku = 4;
SELECT all_fresh('DELETE tb_sku');
TRUNCATE tb_shelf;
SELECT all_fresh('TRUNCATE tb_shelf');

-- Only the readers of what a write changed refresh: an UPDATE of bio, which
-- tv_sku alone reads, leaves the rows of tv_sku_cost and tv_line untouched.
CREATE TEMP TABLE before AS
    SELECT 'cost' AS t, ctid::text AS c FROM tv_sku_cost
    UNION ALL SELECT 'line', ctid::text FROM tv_line;
UPDATE tb_sku SET bio = 'b' WHERE pk_sku = 1;
SELECT all_fresh('UPDATE tb_sku.bio');
SELECT must((SELECT data->>'bio' FROM tv_sku WHERE pk_sku = 1) = 'b', 'tv_sku missed bio');
SELECT must((SELECT count(*) FROM (SELECT 'cost' AS t, ctid::text AS c FROM tv_sku_cost
                                   UNION ALL SELECT 'line', ctid::text FROM tv_line
                                   EXCEPT SELECT * FROM before) x) = 0,
            'an UPDATE of a column only tv_sku reads rewrote rows of another TVIEW');

-- The set stays while a reader is left, and goes with the last one.
SELECT pg_tviews_drop('line');
SELECT must(ours('tb_sku') = 'trg_tview_flush_on_public_tb_sku,trg_tview_row_on_public_tb_sku,'
                             'trg_tview_truncate_on_public_tb_sku',
            'dropping the delta reader keeps the row set: ' || ours('tb_sku'));
SELECT pg_tviews_drop('sku_cost');
SELECT must(ours('tb_sku') = 'trg_tview_flush_on_public_tb_sku,trg_tview_row_on_public_tb_sku,'
                             'trg_tview_truncate_on_public_tb_sku',
            'dropping one row reader keeps the set: ' || ours('tb_sku'));
UPDATE tb_sku SET name = 'S1' WHERE pk_sku = 1;
SELECT assert_fresh('tv_sku', 'pk_sku', 'UPDATE after two drops');
SELECT assert_fresh('tv_shelf', 'pk_shelf', 'UPDATE after two drops');
SELECT pg_tviews_drop('shelf');
SELECT must(ours('tv_sku') = '', 'tv_sku keeps triggers with no reader: ' || ours('tv_sku'));
SELECT pg_tviews_drop('sku');
SELECT must(ours('tb_sku') = '', 'tb_sku keeps triggers with no reader: ' || ours('tb_sku'));

-- A renamed table keeps its one set: a trigger is found by its function, not
-- its name.
SELECT pg_tviews_create('tv_line', $$
    SELECT l.pk_line, l.id, jsonb_build_object('pos', l.pos) AS data FROM tb_line l $$);
ALTER TABLE tb_line RENAME TO tb_line_renamed;
SELECT pg_tviews_create('tv_line_pos', $$
    SELECT l.pk_line AS pk_line_pos, l.id, jsonb_build_object('pos', l.pos) AS data
    FROM tb_line_renamed l $$);
SELECT must((SELECT count(*) FROM pg_trigger WHERE tgrelid = 'tb_line_renamed'::regclass
                                               AND tgname LIKE 'trg_tview_%') = 3,
            'a renamed table got a second set: ' || ours('tb_line_renamed'));
UPDATE tb_line_renamed SET pos = pos + 1;
SELECT assert_fresh('tv_line', 'pk_line', 'UPDATE of a renamed table');
SELECT assert_fresh('tv_line_pos', 'pk_line_pos', 'UPDATE of a renamed table');

-- A partition tree: one row set on the root, one flush and one truncate trigger
-- on each member, however many TVIEWs read the root.
CREATE TABLE tb_ev (pk_ev bigint NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(),
                    kind text, PRIMARY KEY (pk_ev)) PARTITION BY RANGE (pk_ev);
CREATE TABLE tb_ev_1 PARTITION OF tb_ev FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_ev_2 PARTITION OF tb_ev FOR VALUES FROM (100) TO (200);
INSERT INTO tb_ev (pk_ev, kind) VALUES (1, 'a'), (150, 'b');
SELECT pg_tviews_create('tv_ev', $$
    SELECT e.pk_ev, e.id, jsonb_build_object('kind', e.kind) AS data FROM tb_ev e $$);
SELECT pg_tviews_create('tv_ev_kind', $$
    SELECT e.pk_ev AS pk_ev_kind, e.id, jsonb_build_object('k', e.kind) AS data FROM tb_ev e $$);
SELECT must(ours('tb_ev') = 'trg_tview_flush_on_public_tb_ev,trg_tview_row_on_public_tb_ev,'
                            'trg_tview_truncate_on_public_tb_ev',
            'partitioned root: ' || ours('tb_ev'));
SELECT must(ours('tb_ev_1') = 'trg_tview_flush_on_public_tb_ev_1,trg_tview_truncate_on_public_tb_ev_1',
            'partition: ' || ours('tb_ev_1'));
CREATE TABLE tb_ev_3 PARTITION OF tb_ev FOR VALUES FROM (200) TO (300);
SELECT must(ours('tb_ev_3') = 'trg_tview_flush_on_public_tb_ev_3,trg_tview_truncate_on_public_tb_ev_3',
            'new partition: ' || ours('tb_ev_3'));
INSERT INTO tb_ev_3 (pk_ev, kind) VALUES (250, 'c');
UPDATE tb_ev SET kind = upper(kind);
SELECT assert_fresh('tv_ev', 'pk_ev', 'writes to a partition tree');
SELECT assert_fresh('tv_ev_kind', 'pk_ev_kind', 'writes to a partition tree');
ALTER TABLE tb_ev DETACH PARTITION tb_ev_3;
SELECT must(ours('tb_ev_3') = '', 'detached partition keeps triggers: ' || ours('tb_ev_3'));
SELECT assert_fresh('tv_ev', 'pk_ev', 'DETACH');
TRUNCATE tb_ev_2;
SELECT assert_fresh('tv_ev', 'pk_ev', 'TRUNCATE a partition');
SELECT assert_fresh('tv_ev_kind', 'pk_ev_kind', 'TRUNCATE a partition');
SELECT pg_tviews_drop('ev');
SELECT must(ours('tb_ev_1') = 'trg_tview_flush_on_public_tb_ev_1,trg_tview_truncate_on_public_tb_ev_1',
            'one reader left: ' || ours('tb_ev_1'));
SELECT pg_tviews_drop('ev_kind');
SELECT must(ours('tb_ev') || ours('tb_ev_1') || ours('tb_ev_2') = '',
            'no reader left: ' || ours('tb_ev') || ours('tb_ev_1'));

-- Creating a TVIEW over a table needs the TRIGGER privilege on it, also when
-- another TVIEW's set is already there: a reader attaches refreshes to every
-- write of the table.
DROP ROLE IF EXISTS regress_ots_reader;
CREATE ROLE regress_ots_reader;
GRANT USAGE ON SCHEMA tviews TO regress_ots_reader;
CREATE SCHEMA ots AUTHORIZATION regress_ots_reader;
CREATE TABLE tb_acct (pk_acct bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
INSERT INTO tb_acct (pk_acct, name) VALUES (1, 'a');
SELECT pg_tviews_create('tv_acct', $$
    SELECT a.pk_acct, a.id, jsonb_build_object('name', a.name) AS data FROM tb_acct a $$);
GRANT SELECT ON tb_acct TO regress_ots_reader;
SET ROLE regress_ots_reader;
DO $$
BEGIN
    PERFORM tviews.pg_tviews_create('ots.tv_acct_name', $q$
        SELECT a.pk_acct AS pk_acct_name, a.id, jsonb_build_object('n', a.name) AS data
        FROM public.tb_acct a $q$);
    RAISE EXCEPTION 'one trigger set FAIL: a TVIEW over a table without TRIGGER on it was created';
EXCEPTION WHEN insufficient_privilege THEN
    NULL;
END $$;
RESET ROLE;
GRANT TRIGGER ON tb_acct TO regress_ots_reader;
SET ROLE regress_ots_reader;
SELECT tviews.pg_tviews_create('ots.tv_acct_name', $q$
    SELECT a.pk_acct AS pk_acct_name, a.id, jsonb_build_object('n', a.name) AS data
    FROM public.tb_acct a $q$) IS NOT NULL;
RESET ROLE;
SELECT must(ours('tb_acct') = 'trg_tview_flush_on_public_tb_acct,trg_tview_row_on_public_tb_acct,'
                              'trg_tview_truncate_on_public_tb_acct',
            'a reader with TRIGGER shares the set: ' || ours('tb_acct'));
UPDATE tb_acct SET name = 'b';
SELECT assert_fresh('ots.tv_acct_name', 'pk_acct_name', 'UPDATE as the owner');
SELECT pg_tviews_drop('acct_name');
SELECT pg_tviews_drop('acct');
DROP SCHEMA ots CASCADE;
REVOKE ALL ON tb_acct FROM regress_ots_reader;
REVOKE USAGE ON SCHEMA tviews FROM regress_ots_reader;
DROP ROLE regress_ots_reader;

\echo 'one trigger set: PASS'
