-- Regression test: a fan-out patch writes a parent's changed columns into its
-- children only where a recompute would write the same (#288, #289).
--
-- #288: an UPDATE that changes the parent's key pairs no old row image with a
-- new one; the fan-out took that as nothing to write, and the children of the
-- old key kept the parent's values. #289: a condition on the parent besides the
-- join equality (`AND o.deleted_at IS NULL` in a LEFT JOIN's ON clause) decides
-- whether the parent matches at all; the fan-out wrote into the children of a
-- parent the condition excludes. Both were found by the generated shapes
-- (test/sql/differential/gen/).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_fanout_patch_soundness.sql
--
-- expect-output: fan-out patch soundness: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS rekey, filtered CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'fan-out patch soundness FAIL: %', what; END IF; END $$;

-- #288: the parent's key changes, outer and inner join.
CREATE SCHEMA rekey;
CREATE TABLE rekey.tb_org (pk_org bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           amount numeric);
CREATE TABLE rekey.tb_project (pk_project bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                               name text, fk_org bigint);
CREATE TABLE rekey.tb_task (pk_task bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            name text, fk_org bigint);
INSERT INTO rekey.tb_org VALUES (1, DEFAULT, 10), (2, DEFAULT, 20), (3, DEFAULT, 30);
INSERT INTO rekey.tb_project VALUES (1, DEFAULT, 'a', 1), (2, DEFAULT, 'b', 2), (3, DEFAULT, 'c', 3);
INSERT INTO rekey.tb_task VALUES (1, DEFAULT, 'a', 1), (2, DEFAULT, 'b', 2), (3, DEFAULT, 'c', 3);
SELECT tviews.pg_tviews_create('rekey.tv_project', $$
    SELECT t.pk_project, t.id, t.fk_org, jsonb_build_object('name', t.name, 'amount', o.amount) AS data
    FROM rekey.tb_project t LEFT JOIN rekey.tb_org o ON o.pk_org = t.fk_org $$);
SELECT tviews.pg_tviews_create('rekey.tv_task', $$
    SELECT t.pk_task, t.id, t.fk_org, jsonb_build_object('name', t.name, 'amount', o.amount) AS data
    FROM rekey.tb_task t JOIN rekey.tb_org o ON o.pk_org = t.fk_org $$);
UPDATE rekey.tb_org SET pk_org = 1002 WHERE pk_org = 2;
SELECT assert_fresh('rekey.tv_project', 'pk_project', '#288 key change, LEFT JOIN');
SELECT assert_fresh('rekey.tv_task', 'pk_task', '#288 key change, inner join');
-- The key and a copied column at once, and a key change in a statement that also
-- changes another parent's copied column only.
UPDATE rekey.tb_org SET pk_org = 1003, amount = 33 WHERE pk_org = 3;
UPDATE rekey.tb_project SET fk_org = 1003 WHERE pk_project = 1;
UPDATE rekey.tb_org SET pk_org = CASE pk_org WHEN 1 THEN 1001 ELSE pk_org END,
                        amount = amount + 1 WHERE pk_org IN (1, 1003);
SELECT assert_fresh('rekey.tv_project', 'pk_project', '#288 key and value');
SELECT assert_fresh('rekey.tv_task', 'pk_task', '#288 key and value');

-- #289: a condition on the parent in the outer join's ON clause, and in WHERE.
CREATE SCHEMA filtered;
CREATE TABLE filtered.tb_org (pk_org bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                              amount numeric, deleted_at timestamptz);
CREATE TABLE filtered.tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                               name text, fk_org bigint);
CREATE TABLE filtered.tb_note (pk_note bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                               name text, fk_org bigint);
INSERT INTO filtered.tb_org VALUES (1, DEFAULT, 10, NULL), (3, DEFAULT, 30, now());
INSERT INTO filtered.tb_item VALUES (1, DEFAULT, 'a', 1), (3, DEFAULT, 'c', 3);
INSERT INTO filtered.tb_note VALUES (1, DEFAULT, 'a', 1), (3, DEFAULT, 'c', 3);
SELECT tviews.pg_tviews_create('filtered.tv_item', $$
    SELECT t.pk_item, t.id, t.fk_org, jsonb_build_object('name', t.name, 'amount', o.amount) AS data
    FROM filtered.tb_item t LEFT JOIN filtered.tb_org o
      ON o.pk_org = t.fk_org AND o.deleted_at IS NULL $$);
SELECT tviews.pg_tviews_create('filtered.tv_note', $$
    SELECT t.pk_note, t.id, t.fk_org, jsonb_build_object('name', t.name, 'amount', o.amount) AS data
    FROM filtered.tb_note t JOIN filtered.tb_org o ON o.pk_org = t.fk_org
    WHERE o.deleted_at IS NULL $$);
UPDATE filtered.tb_org SET amount = 31 WHERE pk_org = 3;
SELECT assert_fresh('filtered.tv_item', 'pk_item', '#289 a soft-deleted parent, LEFT JOIN');
SELECT assert_fresh('filtered.tv_note', 'pk_note', '#289 a soft-deleted parent, WHERE');
UPDATE filtered.tb_org SET amount = 11 WHERE pk_org = 1;
SELECT assert_fresh('filtered.tv_item', 'pk_item', '#289 a live parent');
SELECT must(data->>'amount' = '11', '#289 the live parent''s child: ' || data::text)
FROM filtered.tv_item WHERE pk_item = 1;

-- A plain join keeps its fan-out: the parent's column is patched, not recomputed.
SELECT must(EXISTS (SELECT 1 FROM jsonb_array_elements(m.plan->'tables') t
                    WHERE t ? 'fanout' AND t->>'table' = 'rekey.tb_org'),
            'the plain join lost its fan-out')
FROM tviews.pg_tview_meta m WHERE m.entity = 'project';
SELECT must(NOT EXISTS (SELECT 1 FROM jsonb_array_elements(m.plan->'tables') t
                        WHERE t ? 'fanout' AND t->>'table' LIKE '%tb_org%'),
            '#289 a parent with a condition of its own kept its fan-out')
FROM tviews.pg_tview_meta m WHERE m.entity IN ('item', 'note');

DROP SCHEMA rekey, filtered CASCADE;
DROP EXTENSION pg_tviews CASCADE;

\echo 'fan-out patch soundness: PASS'
