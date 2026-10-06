-- known-failing: #181
-- Regression test for issue #181: a TVIEW's backing view was always created as
-- <schema>.v_<entity>, so a schema whose own v_<entity> already existed (the
-- application's query view, by the naming convention) could not get the TVIEW.
-- The backing view now lives in the extension's schema, named after the TVIEW's
-- table: tviews.<schema>__<tv table>. The application schema keeps only its own
-- objects.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_181_backing_views.sql
--
-- expect-output: issue #181 backing views: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid UNIQUE NOT NULL DEFAULT gen_random_uuid(),
                       ref text, deleted_at timestamptz);
CREATE TABLE tb_line (pk_line bigint PRIMARY KEY, fk_order bigint REFERENCES tb_order, qty int);
INSERT INTO tb_order VALUES (1, DEFAULT, 'A', NULL), (2, DEFAULT, 'B', NULL);
INSERT INTO tb_line VALUES (1, 1, 3), (2, 1, 4), (3, 2, 5);

-- The application's live view of an order, read by other views.
CREATE VIEW v_order AS SELECT pk_order, id, ref FROM tb_order WHERE deleted_at IS NULL;
CREATE VIEW v_order_with_lines AS
  SELECT o.pk_order, o.id, jsonb_build_object('ref', o.ref,
         'qty', (SELECT sum(qty) FROM tb_line l WHERE l.fk_order = o.pk_order)) AS data
  FROM v_order o;
SELECT pg_catalog.pg_get_viewdef('v_order'::regclass) AS v_order_before \gset

-- ── CREATE TABLE … AS over a view that reads v_order ────────────────────────
DO $$ BEGIN
    CREATE TABLE tv_order AS SELECT pk_order, id, data FROM v_order_with_lines;
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#181 FAIL: CREATE TABLE tv_order AS … refused: %', SQLERRM;
END $$;
UPDATE tb_line SET qty = 10 WHERE pk_line = 1;
SELECT assert_fresh('tv_order', 'pk_order', 'a line update');
UPDATE tb_order SET deleted_at = now() WHERE pk_order = 2;
SELECT assert_fresh('tv_order', 'pk_order', 'an order soft delete');
DO $$ BEGIN
    IF (SELECT view FROM tviews.registry WHERE entity = 'order')::oid
       <> 'tviews.public__tv_order'::regclass::oid THEN
        RAISE EXCEPTION '#181 FAIL: the backing view of tv_order is %',
            (SELECT view::text FROM tviews.registry WHERE entity = 'order');
    END IF;
END $$;
SELECT pg_tviews_drop('tv_order');
DO $$ BEGIN
    IF to_regclass('tviews.public__tv_order') IS NOT NULL THEN
        RAISE EXCEPTION '#181 FAIL: pg_tviews_drop left the backing view';
    END IF;
END $$;

-- ── a TVIEW that is just the application's view, materialized ───────────────
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', 'SELECT * FROM v_order');
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#181 FAIL: pg_tviews_create(''tv_order'', ''SELECT * FROM v_order'') refused: %', SQLERRM;
END $$;
UPDATE tb_order SET ref = 'A2' WHERE pk_order = 1;
SELECT assert_fresh('tv_order', 'pk_order', 'an order update through v_order');

-- The application's view is left alone; the app schema holds no pg_tviews view.
DO $$ BEGIN
    IF pg_catalog.pg_get_viewdef('v_order'::regclass) <> :'v_order_before' THEN
        RAISE EXCEPTION '#181 FAIL: v_order was changed';
    END IF;
    IF EXISTS (SELECT 1 FROM tviews.registry r JOIN pg_class c ON c.oid = r.view::oid
               WHERE c.relnamespace = 'public'::regnamespace) THEN
        RAISE EXCEPTION '#181 FAIL: a backing view is in the application schema';
    END IF;
END $$;

-- ── another schema: the name carries it ─────────────────────────────────────
CREATE SCHEMA app;
CREATE TABLE app.tb_note (pk_note bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), body text);
INSERT INTO app.tb_note (pk_note, body) VALUES (1, 'x');
CREATE TABLE app.tv_note AS SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM app.tb_note;
DO $$ BEGIN
    IF to_regclass('tviews.app__tv_note') IS NULL THEN
        RAISE EXCEPTION '#181 FAIL: the backing view of app.tv_note is %',
            (SELECT view::text FROM tviews.registry WHERE entity = 'note');
    END IF;
END $$;

\echo 'issue #181 backing views: PASS'
