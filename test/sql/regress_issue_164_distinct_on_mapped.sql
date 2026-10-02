-- Regression test for issue #164: a DISTINCT ON TVIEW over a mapped view was
-- refused, and the message counted the wrong tables. A DISTINCT ON TVIEW keyed on
-- pk_<entity> refreshes by that key like any TVIEW, so tables reached through its
-- joins map to it. Keyed on anything else, it is refused with the tables named,
-- unless pg_tviews.uncascaded_policy is full_refresh, which refreshes it in full on
-- writes to them.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_164_distinct_on_mapped.sql
--
-- expect-output: issue #164 DISTINCT ON with mapped tables: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_order (pk_order bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                       id uuid NOT NULL DEFAULT gen_random_uuid(), ref text);
CREATE TABLE tb_line (pk_line bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      fk_order bigint NOT NULL REFERENCES tb_order, sku text);
INSERT INTO tb_order (ref) VALUES ('o1'), ('o2');
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'a'), (1, 'b'), (2, 'c');
CREATE VIEW v_cnt AS SELECT fk_order, count(*) n FROM tb_line GROUP BY fk_order;

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN v_order v USING (pk_order)
               WHERE t.pk_order IS NULL OR v.pk_order IS NULL OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION '#164 FAIL: tv_order stale after %', label;
    END IF;
END $$;

-- ── keyed on pk_<entity>: accepted, and fresh ───────────────────────────────
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', $q$
        SELECT DISTINCT ON (o.pk_order) o.pk_order, o.id, jsonb_build_object('n', v.n) AS data
        FROM tb_order o LEFT JOIN v_cnt v ON v.fk_order = o.pk_order ORDER BY o.pk_order $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#164 FAIL: DISTINCT ON the TVIEW key is refused: %', SQLERRM;
END $$;
INSERT INTO tb_line (fk_order, sku) VALUES (2, 'd');
SELECT check_fresh('an INSERT into tb_line (DISTINCT ON the key)');
DELETE FROM tb_line WHERE sku = 'a';
SELECT check_fresh('a DELETE from tb_line (DISTINCT ON the key)');
UPDATE tb_order SET ref = 'x' WHERE pk_order = 1;
SELECT check_fresh('an UPDATE of tb_order (DISTINCT ON the key)');
DROP TABLE tv_order;

-- ── keyed on another column, under warn: refused, with the table named ──────
DO $$
DECLARE msg text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_create('tv_order', $q$
            SELECT DISTINCT ON (o.id) o.pk_order, o.id, jsonb_build_object('n', v.n) AS data
            FROM tb_order o LEFT JOIN v_cnt v ON v.fk_order = o.pk_order ORDER BY o.id $q$);
        RAISE EXCEPTION '#164 FAIL: DISTINCT ON a non-key column was accepted under warn';
    EXCEPTION WHEN invalid_parameter_value OR raise_exception OR others THEN
        GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT;
    END;
    IF msg LIKE '#164 FAIL%' THEN RAISE EXCEPTION '%', msg; END IF;
    IF msg NOT LIKE '%public.tb_line%' OR msg NOT LIKE '%1 table%' OR msg NOT LIKE '%full_refresh%' THEN
        RAISE EXCEPTION '#164 FAIL: the refusal does not name the table, count it or offer full_refresh: %', msg;
    END IF;
END $$;

-- ── the same under full_refresh: accepted, refreshed in full ────────────────
SET pg_tviews.uncascaded_policy = 'full_refresh';
SELECT pg_tviews_create('tv_order', $$
    SELECT DISTINCT ON (o.id) o.pk_order, o.id, jsonb_build_object('n', v.n) AS data
    FROM tb_order o LEFT JOIN v_cnt v ON v.fk_order = o.pk_order ORDER BY o.id $$);
RESET pg_tviews.uncascaded_policy;
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'e');
SELECT check_fresh('an INSERT into tb_line (full_refresh)');
UPDATE tb_order SET ref = 'y' WHERE pk_order = 2;
SELECT check_fresh('an UPDATE of tb_order (full_refresh)');

-- ── keyed on a unique NOT NULL column of the root table: accepted under warn ─
DROP TABLE tv_order;
ALTER TABLE tb_order ADD COLUMN code text;
UPDATE tb_order SET code = 'C' || pk_order;
ALTER TABLE tb_order ALTER COLUMN code SET NOT NULL, ADD CONSTRAINT tb_order_code_key UNIQUE (code);
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', $q$
        SELECT DISTINCT ON (o.code) o.pk_order, o.id, o.code, jsonb_build_object('n', v.n) AS data
        FROM tb_order o LEFT JOIN v_cnt v ON v.fk_order = o.pk_order ORDER BY o.code $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#164 FAIL: DISTINCT ON a unique NOT NULL column is refused: %', SQLERRM;
END $$;
INSERT INTO tb_line (fk_order, sku) VALUES (2, 'f');
SELECT check_fresh('an INSERT into tb_line (DISTINCT ON a unique column)');
DELETE FROM tb_line WHERE sku = 'f';
SELECT check_fresh('a DELETE from tb_line (DISTINCT ON a unique column)');
UPDATE tb_order SET ref = 'z' WHERE pk_order = 1;
SELECT check_fresh('an UPDATE of tb_order (DISTINCT ON a unique column)');
INSERT INTO tb_order (ref, code) VALUES ('o3', 'C3');
SELECT check_fresh('an INSERT into tb_order (DISTINCT ON a unique column)');
DO $$ BEGIN
    IF (SELECT cascade_kinds::text FROM tviews.registry WHERE entity = 'order') LIKE '%all_keys%' THEN
        RAISE EXCEPTION '#164 FAIL: a table read through the join is all_keys: %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'order');
    END IF;
END $$;
SELECT count(*) FROM pg_tviews_reregister_all();
INSERT INTO tb_line (fk_order, sku) VALUES (3, 'g');
SELECT check_fresh('an INSERT into tb_line after re-registration');

\echo 'issue #164 DISTINCT ON with mapped tables: PASS'
