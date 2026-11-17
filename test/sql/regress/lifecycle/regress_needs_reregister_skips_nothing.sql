-- A TVIEW whose plan an update could not derive again (needs_reregister) is
-- refreshed on every write to its tables: no UPDATE is skipped on the columns its
-- plan names, and none is patched into its document. Its plan is an earlier
-- release's, and what it says the TVIEW reads is not trusted.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lifecycle/regress_needs_reregister_skips_nothing.sql
--
-- expect-output: needs_reregister skips nothing: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

CREATE TABLE tb_cat (pk_cat int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     label text, note text);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_cat int NOT NULL REFERENCES tb_cat, name text, note text);
INSERT INTO tb_cat (pk_cat, label) VALUES (1, 'c1');
INSERT INTO tb_item (pk_item, fk_cat, name) VALUES (1, 1, 'i1'), (2, 1, 'i2');
-- name is copied (a direct patch), label is fanned out, note is read by neither.
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, i.fk_cat, jsonb_build_object('name', i.name, 'cat', c.label) AS data
    FROM tb_item i JOIN tb_cat c ON c.pk_cat = i.fk_cat $$) IS NOT NULL AS created;

-- The work one write did: its counters, read in its own transaction around it
-- (the session's counters are kept per transaction).
CREATE TABLE seen (q jsonb);
CREATE FUNCTION mark() RETURNS void LANGUAGE sql AS $$
    DELETE FROM seen; INSERT INTO seen SELECT tviews.pg_tviews_show_queue() $$;
CREATE FUNCTION work() RETURNS jsonb LANGUAGE sql AS $$
    SELECT jsonb_build_object(
        'recomputes', (n->>'view_recomputes')::int - (s.q->>'view_recomputes')::int,
        'captured', (n->>'direct_patch_captured')::int - (s.q->>'direct_patch_captured')::int)
    FROM seen s, tviews.pg_tviews_show_queue() AS now(n) $$;

-- With a plan of this release: an unread column is skipped, a copied one is
-- patched, a fanned-out one is written into the rows that copy it.
BEGIN; SELECT mark(); UPDATE tb_item SET note = 'a' WHERE pk_item = 1;
SELECT must(work()->>'recomputes' = '0', 'an UPDATE of an unread column of tb_item recomputed rows');
COMMIT;
BEGIN; SELECT mark(); UPDATE tb_cat SET note = 'a' WHERE pk_cat = 1;
SELECT must(work()->>'recomputes' = '0', 'an UPDATE of an unread column of tb_cat recomputed rows');
COMMIT;
BEGIN; SELECT mark(); UPDATE tb_item SET name = 'i1b' WHERE pk_item = 1;
SELECT must(work() = '{"recomputes": 0, "captured": 1}', 'an UPDATE of a copied column was not patched');
COMMIT;
BEGIN; SELECT mark(); UPDATE tb_cat SET label = 'c1b' WHERE pk_cat = 1;
SELECT must(work()->>'recomputes' = '0', 'an UPDATE of a fanned-out column was recomputed');
COMMIT;
SELECT assert_fresh('tv_item', 'pk_item', 'the writes with a current plan');

-- The update could not derive the plan again.
UPDATE tviews.pg_tview_meta SET needs_reregister = true WHERE entity = 'item';

BEGIN; SELECT mark(); UPDATE tb_item SET note = 'b' WHERE pk_item = 1;
SELECT must(work()->>'recomputes' = '1', 'an UPDATE of an unread column of tb_item was skipped');
COMMIT;
BEGIN; SELECT mark(); UPDATE tb_cat SET note = 'b' WHERE pk_cat = 1;
SELECT must(work()->>'recomputes' = '2', 'an UPDATE of an unread column of tb_cat was skipped');
COMMIT;
BEGIN; SELECT mark(); UPDATE tb_item SET name = 'i1c' WHERE pk_item = 1;
SELECT must(work() = '{"recomputes": 1, "captured": 0}', 'an UPDATE of a copied column was patched');
COMMIT;
BEGIN; SELECT mark(); UPDATE tb_cat SET label = 'c1c' WHERE pk_cat = 1;
SELECT must(work()->>'recomputes' = '2', 'an UPDATE of a fanned-out column was patched');
COMMIT;
SELECT assert_fresh('tv_item', 'pk_item', 'the writes with an earlier release''s plan');

-- Re-registered, it skips again.
SELECT tviews.pg_tviews_reregister('item') IS NOT NULL AS reregistered;
BEGIN; SELECT mark(); UPDATE tb_item SET note = 'c' WHERE pk_item = 1;
SELECT must(work()->>'recomputes' = '0', 'an UPDATE of an unread column recomputed rows after the re-registration');
COMMIT;

SELECT 'needs_reregister skips nothing: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
