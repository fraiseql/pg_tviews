-- A table read by a sub-select inside a VALUES list (LATERAL (VALUES (...))) is
-- read there too: a TVIEW that also joins the table directly, on another row,
-- refreshes the rows whose VALUES list reads a written row.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_read_in_lateral_values.sql
--
-- known-failing: #321 (walker)
-- expect-output: read in a VALUES list: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_lbl (pk_lbl int PRIMARY KEY, label text, n int);
CREATE TABLE tb_t (pk_t int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                   fk_lbl int NOT NULL REFERENCES tb_lbl, name text);
INSERT INTO tb_lbl VALUES (1, 'one', 1), (2, 'two', 2), (3, 'three', 3);
INSERT INTO tb_t (pk_t, fk_lbl, name) VALUES (1, 1, 'a'), (2, 2, 'b'), (3, 1, 'c');
SELECT tviews.pg_tviews_create('tv_t', $$
    SELECT t.pk_t, t.id, jsonb_build_object('name', t.name, 'n', d.n, 'label', v.l) AS data
    FROM tb_t t JOIN tb_lbl d ON d.pk_lbl = t.fk_lbl CROSS JOIN LATERAL (VALUES ((SELECT l.label FROM tb_lbl l WHERE l.pk_lbl = t.pk_t))) AS v(l) $$) IS NOT NULL AS created;

UPDATE tb_lbl SET label = 'tres' WHERE pk_lbl = 3;
SELECT assert_fresh('tv_t', 'pk_t', 'an UPDATE of a table read inside a VALUES list');

SELECT 'read in a VALUES list: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
