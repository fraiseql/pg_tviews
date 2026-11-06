-- A row key that is not an integer is read from the written row as text under
-- the settings every refresh runs under, not the writer's: the refresh looks
-- the key up again under those settings.
--
-- A float key under the writer's extra_float_digits, or a date key under its
-- DateStyle, read as text there and parsed back under the pinned settings, would
-- name another value or none, and the TVIEW row of the written key would go
-- stale.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/identity/regress_key_render_settings.sql
-- expect-output: key render settings: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_byscore (pk_byscore int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         score float8 NOT NULL, rev int NOT NULL, label text);
CREATE TABLE tb_byday (pk_byday int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       day date NOT NULL, rev int NOT NULL, label text);
INSERT INTO tb_byscore VALUES (1, DEFAULT, 0.1::float8 + 0.2::float8, 1, 'a'),
                              (2, DEFAULT, 0.5, 2, 'b');
INSERT INTO tb_byday VALUES (1, DEFAULT, '2026-12-05', 1, 'a'), (2, DEFAULT, '2026-01-31', 2, 'b');
SELECT tviews.pg_tviews_create('tv_byscore', $$
  SELECT DISTINCT ON (i.score) i.pk_byscore, i.id, i.score,
         jsonb_build_object('label', i.label) AS data
  FROM tb_byscore i ORDER BY i.score, i.rev DESC $$);
SELECT tviews.pg_tviews_create('tv_byday', $$
  SELECT DISTINCT ON (i.day) i.pk_byday, i.id, i.day,
         jsonb_build_object('label', i.label) AS data
  FROM tb_byday i ORDER BY i.day, i.rev DESC $$);

-- The writer's settings.
SET extra_float_digits = 0;
SET DateStyle = 'SQL, DMY';
UPDATE tb_byscore SET label = 'a2' WHERE pk_byscore = 1;
INSERT INTO tb_byscore VALUES (3, DEFAULT, 0.1::float8 + 0.2::float8, 3, 'c');
UPDATE tb_byday SET label = 'a2' WHERE pk_byday = 1;
INSERT INTO tb_byday VALUES (3, DEFAULT, '2026-12-05', 3, 'c');
RESET extra_float_digits;
RESET DateStyle;

SELECT assert_fresh('tv_byscore', 'score', 'writes under extra_float_digits 0');
SELECT assert_fresh('tv_byday', 'day', 'writes under DateStyle SQL, DMY');

SELECT 'key render settings: PASS' AS result;
