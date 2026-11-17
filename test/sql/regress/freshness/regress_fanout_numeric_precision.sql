-- A numeric copied into the documents that embed its row (a fan-out patch) keeps
-- every digit: 30 significant digits are not rounded to a double's 17.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_fanout_numeric_precision.sql
--
-- known-failing: #321 (G2)
-- expect-output: fan-out numeric precision: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_cur (pk_cur int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), rate numeric);
CREATE TABLE tb_inv (pk_inv int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     fk_cur int NOT NULL REFERENCES tb_cur, ref text);
INSERT INTO tb_cur (pk_cur, rate) VALUES (1, 1.50);
INSERT INTO tb_inv (pk_inv, fk_cur, ref) VALUES (1, 1, 'x'), (2, 1, 'y');
SELECT tviews.pg_tviews_create('tv_inv', $$
    SELECT i.pk_inv, i.id, i.fk_cur,
           jsonb_build_object('ref', i.ref, 'cur', jsonb_build_object('rate', c.rate)) AS data
    FROM tb_inv i JOIN tb_cur c ON c.pk_cur = i.fk_cur $$) IS NOT NULL AS created;

UPDATE tb_cur SET rate = 1.123456789012345678901234567890 WHERE pk_cur = 1;
SELECT assert_fresh('tv_inv', 'pk_inv', 'an UPDATE of a numeric with 30 digits, copied by fan-out');

SELECT 'fan-out numeric precision: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
