-- A numeric copied by a fan-out patch keeps its scale: 2.50 stays 2.50, as the
-- view renders it. jsonb equality calls 2.5 and 2.50 equal; the stored text, and
-- every reader of it, does not.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_fanout_numeric_scale.sql
--
-- known-failing: #321 (G2)
-- expect-output: fan-out numeric scale: PASS

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

CREATE TABLE tb_cur (pk_cur int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), rate numeric);
CREATE TABLE tb_inv (pk_inv int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     fk_cur int NOT NULL REFERENCES tb_cur, ref text);
INSERT INTO tb_cur (pk_cur, rate) VALUES (1, 1.25);
INSERT INTO tb_inv (pk_inv, fk_cur, ref) VALUES (1, 1, 'x'), (2, 1, 'y');
SELECT tviews.pg_tviews_create('tv_inv', $$
    SELECT i.pk_inv, i.id, i.fk_cur,
           jsonb_build_object('ref', i.ref, 'cur', jsonb_build_object('rate', c.rate)) AS data
    FROM tb_inv i JOIN tb_cur c ON c.pk_cur = i.fk_cur $$) IS NOT NULL AS created;

UPDATE tb_cur SET rate = 2.50 WHERE pk_cur = 1;
-- The jsonb-equal comparison sees no difference: the case the oracle missed.
SELECT must(NOT EXISTS (SELECT data FROM tv_inv EXCEPT SELECT data FROM tviews.public__tv_inv),
            'jsonb equality calls the stored and the viewed document equal');
SELECT assert_fresh('tv_inv', 'pk_inv', 'an UPDATE of a numeric to 2.50, copied by fan-out');

SELECT 'fan-out numeric scale: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
