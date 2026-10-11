-- Regression test for issues #70 and #73:
--   #70 "Default GIN index on data makes every refresh non-HOT"
--   #73 "TVIEW tables use fillfactor 100, so refreshes spill to new pages"
--
-- Nearly every refresh rewrites `data`. A GIN index on `data` made every refresh
-- a non-HOT update (new entries in every index, a dead tuple needing index
-- cleanup, a cleared visibility-map bit), and with fillfactor 100 even an
-- index-compatible update often found no room on its page.
--
-- Correct behaviour: new TVIEWs (pg_tviews_create and CREATE TABLE tv_* AS) get
-- no GIN on `data` unless their options say `data_gin_index: true`, and are
-- created WITH (fillfactor = 85) unless their options say otherwise, so
-- single-row refreshes stay heap-only.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_hot_defaults.sql
-- expect-output: hot_defaults: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_post, tv_comment, tv_tag CASCADE;
DROP VIEW  IF EXISTS tviews.public__tv_post, tviews.public__tv_comment, tviews.public__tv_tag CASCADE;
DROP TABLE IF EXISTS tb_comment, tb_post, tb_tag CASCADE;

CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title   TEXT
);
CREATE TABLE tb_comment (
    pk_comment INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_post    INTEGER REFERENCES tb_post(pk_post),
    body       TEXT
);
CREATE TABLE tb_tag (
    pk_tag INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id     UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    label  TEXT
);
INSERT INTO tb_post (title) SELECT 'post ' || g FROM generate_series(1, 2000) g;
INSERT INTO tb_comment (fk_post, body) SELECT g, 'comment ' || g FROM generate_series(1, 2000) g;
INSERT INTO tb_tag (label) VALUES ('a'), ('b');

CREATE FUNCTION pg_temp.has_gin(tbl regclass) RETURNS boolean LANGUAGE sql AS $$
    SELECT EXISTS (SELECT 1 FROM pg_index i JOIN pg_class ic ON ic.oid = i.indexrelid
                   JOIN pg_am am ON am.oid = ic.relam
                   WHERE i.indrelid = tbl AND am.amname = 'gin')
$$;
CREATE FUNCTION pg_temp.fillfactor(tbl regclass) RETURNS text LANGUAGE sql AS $$
    SELECT option_value FROM pg_options_to_table((SELECT reloptions FROM pg_class WHERE oid = tbl))
    WHERE option_name = 'fillfactor'
$$;

-- ── Defaults, both creation paths ───────────────────────────────────────────
SELECT pg_tviews_create('tv_post', $TV$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post
$TV$);
CREATE TABLE tv_comment AS
    SELECT pk_comment, id, fk_post, jsonb_build_object('body', body) AS data FROM tb_comment;

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['tv_post', 'tv_comment'] LOOP
        IF pg_temp.has_gin(t::regclass) THEN
            RAISE EXCEPTION 'FAIL #70: % has a GIN index on data by default', t;
        END IF;
        IF pg_temp.fillfactor(t::regclass) IS DISTINCT FROM '85' THEN
            RAISE EXCEPTION 'FAIL #73: % fillfactor is %, expected 85', t, pg_temp.fillfactor(t::regclass);
        END IF;
    END LOOP;
END $$;

-- ── Single-row refreshes stay heap-only ─────────────────────────────────────
-- One statement per row, so each flush refreshes exactly one tview row.
SELECT pg_stat_force_next_flush();
SELECT pg_stat_reset_single_table_counters('tv_post'::regclass);
SELECT pg_stat_reset_single_table_counters('tv_comment'::regclass);
SELECT format('UPDATE tb_post SET title = %L WHERE pk_post = %s', 'edited ' || g, g)
FROM generate_series(1, 2000) g \gexec
SELECT format('UPDATE tb_comment SET body = %L WHERE pk_comment = %s', 'edited ' || g, g)
FROM generate_series(1, 2000) g \gexec
SELECT pg_stat_force_next_flush();

DO $$
DECLARE r record;
BEGIN
    FOR r IN SELECT relname, n_tup_upd, n_tup_hot_upd FROM pg_stat_user_tables
             WHERE relname IN ('tv_post', 'tv_comment') LOOP
        -- tv_comment also receives the post edits through its fk_post link, so
        -- it sees at least its own 2000 refreshes; all of them should be HOT.
        IF r.n_tup_upd < 2000 THEN
            RAISE EXCEPTION 'setup: % got % updates, expected >= 2000', r.relname, r.n_tup_upd;
        END IF;
        IF r.n_tup_hot_upd::float / r.n_tup_upd < 0.95 THEN
            RAISE EXCEPTION 'FAIL #70/#73: % HOT ratio % (% of %), expected >= 95%%',
                r.relname, round(100.0 * r.n_tup_hot_upd / r.n_tup_upd, 1), r.n_tup_hot_upd, r.n_tup_upd;
        END IF;
    END LOOP;
END $$;

-- ── Per-TVIEW opt-out through its options ───────────────────────────────────
SELECT pg_tviews_create('tv_tag', $TV$
    SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM tb_tag
$TV$, '{"data_gin_index": true, "fillfactor": 100}');

DO $$ BEGIN
    IF NOT pg_temp.has_gin('tv_tag') THEN
        RAISE EXCEPTION 'FAIL #70: data_gin_index: true did not create the GIN index';
    END IF;
    IF (SELECT reloptions FROM pg_class WHERE oid = 'tv_tag'::regclass) IS NOT NULL THEN
        RAISE EXCEPTION 'FAIL #73: fillfactor = 100 should leave reloptions empty, got %',
            (SELECT reloptions FROM pg_class WHERE oid = 'tv_tag'::regclass);
    END IF;
END $$;

-- ── Refreshes are still correct ─────────────────────────────────────────────
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post) WHERE t.data IS DISTINCT FROM v.data)
       OR EXISTS (SELECT 1 FROM tv_comment t FULL JOIN tviews.public__tv_comment v USING (pk_comment) WHERE t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'FAIL: a tview diverges from its backing view';
    END IF;
END $$;

\echo 'hot_defaults: PASS'
