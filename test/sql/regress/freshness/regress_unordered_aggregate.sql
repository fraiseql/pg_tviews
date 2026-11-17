-- An aggregate whose output depends on the order of its input rows, with no
-- ORDER BY to fix it, is refused at creation (0A000): an UPDATE that moves a row
-- in the heap, even of a column the definition does not read, changes what the
-- view returns, and nothing would refresh the stored document.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_unordered_aggregate.sql
--
-- known-failing: #321 (order, D13)
-- expect-output: unordered aggregate: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_p (pk_p int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_c (pk_c int PRIMARY KEY, fk_p int NOT NULL REFERENCES tb_p, label text, note text);
INSERT INTO tb_p (pk_p, name) VALUES (1, 'p');
INSERT INTO tb_c VALUES (1, 1, 'a', NULL), (2, 1, 'b', NULL);

DO $$
DECLARE state text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_create('tv_p', $d$
            SELECT p.pk_p, p.id,
                   jsonb_build_object('labels', (SELECT jsonb_agg(c.label) FROM tb_c c WHERE c.fk_p = p.pk_p)) AS data
            FROM tb_p p $d$);
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE;
        IF state <> '0A000' THEN
            RAISE EXCEPTION 'FAIL: an unordered jsonb_agg was refused with % (want 0A000): %', state, SQLERRM;
        END IF;
        RETURN;
    END;
    -- Accepted: show what it costs.
    UPDATE tb_c SET note = 'x' WHERE pk_c = 1;
    RAISE EXCEPTION 'FAIL: an unordered jsonb_agg was accepted; after an UPDATE of an unread column: %',
        coalesce(fresh_diff('tv_p', 'pk_p'), 'still fresh');
END $$;

SELECT 'unordered aggregate: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
