-- Regression test for issue #163: a view whose CTEs read each other three or more
-- deep, or that defines a CTE it never uses, was refused at create ("not found in
-- the view's query"). A CTE's references count levels from where it is defined;
-- the analyzer counted them from the level it was walking.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_163_cte_levels.sql
--
-- expect-output: issue #163 CTE levels: PASS

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
CREATE TABLE tb_tag (pk_tag bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, sku text, tag text);
INSERT INTO tb_order (ref) VALUES ('o1'), ('o2');
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'a'), (1, 'b'), (2, 'c');
INSERT INTO tb_tag (sku, tag) VALUES ('a', 't1'), ('c', 't2');

CREATE FUNCTION check_fresh(tv text, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE d bigint;
BEGIN
    EXECUTE format(
        'SELECT count(*) FROM ((SELECT pk_order, data FROM %1$I EXCEPT SELECT pk_order, data FROM %2$I)
                     UNION ALL (SELECT pk_order, data FROM %2$I EXCEPT SELECT pk_order, data FROM %1$I)) d',
        tv, 'v_' || substr(tv, 4)) INTO d;
    IF d <> 0 THEN
        RAISE EXCEPTION '#163 FAIL: % stale after %', tv, label;
    END IF;
END $$;

-- ── three CTEs, each reading the previous one ───────────────────────────────
CREATE VIEW v_k3 AS
  WITH c1 AS (SELECT sku, tag FROM tb_tag), c2 AS (SELECT sku, tag FROM c1), c3 AS (SELECT sku, tag FROM c2)
  SELECT l.fk_order, count(c3.tag) n FROM tb_line l LEFT JOIN c3 USING (sku) GROUP BY l.fk_order;
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', $q$
        SELECT o.pk_order, o.id, jsonb_build_object('n', v.n) AS data
        FROM tb_order o LEFT JOIN v_k3 v ON v.fk_order = o.pk_order $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#163 FAIL: a chain of three CTEs is refused: %', SQLERRM;
END $$;
INSERT INTO tb_tag (sku, tag) VALUES ('b', 't3');
SELECT check_fresh('tv_order', 'INSERT into the table the CTE chain reads');
UPDATE tb_tag SET sku = 'zz' WHERE tag = 't1';
SELECT check_fresh('tv_order', 'UPDATE of the table the CTE chain reads');

-- ── ten CTEs ────────────────────────────────────────────────────────────────
CREATE VIEW v_k10 AS
  WITH c1 AS (SELECT sku, tag FROM tb_tag), c2 AS (SELECT * FROM c1), c3 AS (SELECT * FROM c2),
       c4 AS (SELECT * FROM c3), c5 AS (SELECT * FROM c4), c6 AS (SELECT * FROM c5),
       c7 AS (SELECT * FROM c6), c8 AS (SELECT * FROM c7), c9 AS (SELECT * FROM c8),
       c10 AS (SELECT * FROM c9)
  SELECT l.fk_order, count(c10.tag) n FROM tb_line l LEFT JOIN c10 USING (sku) GROUP BY l.fk_order;
DROP TABLE tv_order;
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', $q$
        SELECT o.pk_order, o.id, jsonb_build_object('n', v.n) AS data
        FROM tb_order o LEFT JOIN v_k10 v ON v.fk_order = o.pk_order $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#163 FAIL: a chain of ten CTEs is refused: %', SQLERRM;
END $$;
UPDATE tb_tag SET tag = 't1b' WHERE tag = 't1';
SELECT check_fresh('tv_order', 'UPDATE under ten CTEs');

-- ── a CTE used only by a CTE defined in a subquery ──────────────────────────
CREATE VIEW v_nested AS
  WITH outer_tags AS (SELECT sku, tag FROM tb_tag)
  SELECT s.fk_order, s.n FROM (
      WITH inner_tags AS (SELECT sku, tag FROM outer_tags)
      SELECT l.fk_order, count(i.tag) n FROM tb_line l LEFT JOIN inner_tags i USING (sku)
      GROUP BY l.fk_order) s;
DROP TABLE tv_order;
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', $q$
        SELECT o.pk_order, o.id, jsonb_build_object('n', v.n) AS data
        FROM tb_order o LEFT JOIN v_nested v ON v.fk_order = o.pk_order $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#163 FAIL: a CTE read from a nested CTE is refused: %', SQLERRM;
END $$;
INSERT INTO tb_tag (sku, tag) VALUES ('c', 't4');
SELECT check_fresh('tv_order', 'INSERT under a nested CTE');

-- ── a CTE the view never uses ───────────────────────────────────────────────
CREATE TABLE tb_item (pk_item bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_unused (k int);
INSERT INTO tb_item (name) VALUES ('i1');
CREATE VIEW v_f AS WITH unused AS (SELECT * FROM tb_unused) SELECT pk_item, name FROM tb_item;
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_item', $q$
        SELECT i.pk_item, i.id, jsonb_build_object('name', v.name) AS data
        FROM tb_item i JOIN v_f v USING (pk_item) $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#163 FAIL: a view with an unused CTE is refused: %', SQLERRM;
END $$;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'tb_unused'::regclass AND tgname LIKE 'trg_tview_%')
       OR (SELECT cascade_kinds ? 'tb_unused' FROM tviews.registry WHERE entity = 'item') THEN
        RAISE EXCEPTION '#163 FAIL: a table read only by an unused CTE is tracked';
    END IF;
END $$;

DO $$ BEGIN
    IF (SELECT status FROM tviews.pg_tviews_health_check() WHERE component = 'triggers') <> 'OK' THEN
        RAISE EXCEPTION '#163 FAIL: health check: %',
            (SELECT message FROM tviews.pg_tviews_health_check() WHERE component = 'triggers');
    END IF;
END $$;

\echo 'issue #163 CTE levels: PASS'
