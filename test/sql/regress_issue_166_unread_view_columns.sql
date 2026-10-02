-- Regression test for issue #166: a table that feeds only view output columns the
-- TVIEW never reads was still mapped, so every write to it mapped its rows to keys
-- and recomputed them for nothing. Columns no level above reads are skipped; the
-- tables behind them are known but not tracked.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_166_unread_view_columns.sql
--
-- expect-output: issue #166 unread view columns: PASS

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

CREATE VIEW v_stats AS SELECT o.pk_order,
  (SELECT count(*) FROM tb_line l WHERE l.fk_order = o.pk_order) AS n_lines,
  (SELECT max(t.tag) FROM tb_tag t JOIN tb_line l ON l.sku = t.sku WHERE l.fk_order = o.pk_order) AS last_tag
  FROM tb_order o;

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN v_order v USING (pk_order)
               WHERE t.pk_order IS NULL OR v.pk_order IS NULL OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION '#166 FAIL: tv_order stale after %', label;
    END IF;
END $$;

-- ── last_tag is never read: tb_tag is not tracked ───────────────────────────
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, jsonb_build_object('n', v.n_lines) AS data
  FROM tb_order o JOIN v_stats v USING (pk_order) $$);
DO $$ BEGIN
    IF (SELECT cascade_kinds ? 'tb_tag' FROM tviews.registry WHERE entity = 'order')
       OR EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'tb_tag'::regclass AND tgname LIKE 'trg_tview_%') THEN
        RAISE EXCEPTION '#166 FAIL: tb_tag, read only by the unread column last_tag, is tracked: %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'order');
    END IF;
END $$;
INSERT INTO tb_line (fk_order, sku) VALUES (2, 'd');
SELECT check_fresh('an INSERT into tb_line (n_lines is read)');
DO $$ BEGIN
    IF (SELECT status FROM tviews.pg_tviews_health_check() WHERE component = 'triggers') <> 'OK' THEN
        RAISE EXCEPTION '#166 FAIL: health check: %',
            (SELECT message FROM tviews.pg_tviews_health_check() WHERE component = 'triggers');
    END IF;
END $$;
DROP TABLE tv_order;

-- ── reading last_tag brings tb_tag back ─────────────────────────────────────
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, jsonb_build_object('n', v.n_lines, 'tag', v.last_tag) AS data
  FROM tb_order o JOIN v_stats v USING (pk_order) $$);
UPDATE tb_tag SET tag = 't9' WHERE sku = 'a';
SELECT check_fresh('an UPDATE of tb_tag (last_tag is read)');
DROP TABLE tv_order;

-- ── a column read only in WHERE, or through a whole-row reference, counts ───
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, jsonb_build_object('ref', o.ref) AS data
  FROM tb_order o JOIN v_stats v USING (pk_order) WHERE v.last_tag IS NOT NULL $$);
UPDATE tb_tag SET tag = NULL WHERE sku = 'c';
SELECT check_fresh('an UPDATE of tb_tag (last_tag read in WHERE)');
DROP TABLE tv_order;
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, to_jsonb(v) AS data
  FROM tb_order o JOIN v_stats v USING (pk_order) $$);
UPDATE tb_tag SET tag = 't8' WHERE sku = 'c';
SELECT check_fresh('an UPDATE of tb_tag (whole-row reference)');

\echo 'issue #166 unread view columns: PASS'
