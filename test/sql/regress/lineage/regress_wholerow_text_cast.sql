-- A row cast to text is read whole, also through a join alias, which reads every
-- table under the join whole.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_wholerow_text_cast.sql
--
-- expect-output: whole row cast to text: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- The refreshes forced because the definition reads a table whole, by reason
-- (test-hooks builds; NULL in others).
CREATE FUNCTION stopgap(reason text) RETURNS bigint LANGUAGE sql AS $$
    SELECT CASE WHEN tviews.pg_tviews_show_queue() ? 'patch_stats'
                THEN coalesce((tviews.pg_tviews_show_queue()->'patch_stats'->>('stopgap:' || reason))::bigint, 0)
           END $$;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

CREATE TABLE tb_w (pk_w int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), a int, b int);
CREATE TABLE tb_wx (pk_wx int PRIMARY KEY, fk_w int NOT NULL REFERENCES tb_w, c int);
INSERT INTO tb_w (pk_w, a, b) VALUES (1, 1, 1), (2, 2, 2);
INSERT INTO tb_wx (pk_wx, fk_w, c) VALUES (1, 1, 1), (2, 2, 2);
SELECT tviews.pg_tviews_create('tv_w', $$
    SELECT w.pk_w, w.id, jsonb_build_object('hash', md5(w::text)) AS data FROM tb_w w $$) IS NOT NULL AS created;
-- The join alias j stands for both tables.
SELECT tviews.pg_tviews_create('tv_wj', $$
    SELECT w.pk_w AS pk_wj, w.id, jsonb_build_object('hash',
        (SELECT md5(j::text) FROM (tb_w w2 JOIN tb_wx x ON x.fk_w = w2.pk_w) AS j
         WHERE j.pk_w = w.pk_w ORDER BY j.pk_wx LIMIT 1)) AS data
    FROM tb_w w $$) IS NOT NULL AS created;

UPDATE tb_w SET b = 20 WHERE pk_w = 1;
SELECT assert_fresh('tv_w', 'pk_w', 'an UPDATE of a column read only through the row cast to text');
SELECT assert_fresh('tv_wj', 'pk_wj', 'the same UPDATE, read through a join alias');
UPDATE tb_wx SET c = 30 WHERE pk_wx = 2;
SELECT assert_fresh('tv_wj', 'pk_wj', 'an UPDATE of the joined table, read through the join alias');

SELECT must(stopgap('wholerow') IS NULL OR stopgap('wholerow') > 0, 'no stopgap:wholerow counted');

SELECT 'whole row cast to text: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
