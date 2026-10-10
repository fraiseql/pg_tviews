-- Regression test for issue #74: pg_tviews_profile(), per-TVIEW physical health.
--
-- One read-only report of what refreshes cost PostgreSQL per TVIEW (sizes, HOT
-- ratio, dead tuples, indexes, fan-out), with a warning for each known pathology.
-- Each warning rule gets a case that triggers it and one that does not.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_profile.sql
-- expect-output: profile: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- Statistics reach the views asynchronously: force the flush, then read fresh.
CREATE FUNCTION settle() RETURNS void LANGUAGE sql AS $$ SELECT pg_stat_force_next_flush() $$;
CREATE FUNCTION warned(e TEXT, pattern TEXT, warn BIGINT DEFAULT 1000) RETURNS BOOLEAN
LANGUAGE sql AS $$
    SELECT pg_stat_clear_snapshot();
    SELECT EXISTS (
        SELECT 1 FROM pg_tviews_profile(e, warn) p, unnest(p.warnings) w WHERE w LIKE pattern)
$$;
CREATE FUNCTION must(ok BOOLEAN, msg TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION '#74 FAIL: %', msg; END IF; END $$;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user BIGINT NOT NULL,
    title   TEXT
);
INSERT INTO tb_user (name) SELECT 'u' || g FROM generate_series(1, 50) g;
-- User 1 has 3000 posts, the other 49 about 40 each.
INSERT INTO tb_post (fk_user, title)
SELECT CASE WHEN g <= 3000 THEN 1 ELSE 2 + g % 49 END, 't' || g FROM generate_series(1, 5000) g;

CREATE UNLOGGED TABLE tv_user AS
SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user;
CREATE UNLOGGED TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.name) AS data
FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user;
CREATE TABLE tb_tag (
    pk_tag BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id     UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    label  TEXT
);
INSERT INTO tb_tag (label) SELECT 'l' || g FROM generate_series(1, 100) g;
CREATE TABLE tv_tag AS SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM tb_tag;
ANALYZE tv_user, tv_post, tv_tag;

-- ========================================================================
-- Cycle 1: shape
-- ========================================================================
SELECT must((SELECT array_agg(entity ORDER BY entity) FROM pg_tviews_profile()) = ARRAY['post', 'tag', 'user'],
            'one row per TVIEW');
SELECT must((SELECT heap_bytes > 0 AND index_bytes > 0 AND rows_estimate = 5000
                    AND fillfactor = 85 AND tview = 'public.tv_post'
             FROM pg_tviews_profile('post')), 'sizes / estimates / fillfactor of tv_post');

-- ========================================================================
-- W7 UNLOGGED, W6 fan-out
-- ========================================================================
SELECT must(warned('post', 'UNLOGGED:%'), 'W7 on an UNLOGGED TVIEW');
SELECT must(NOT warned('tag', 'UNLOGGED:%'), 'W7 on a LOGGED TVIEW');
SELECT must((SELECT (fanout->'fk_user'->>'max')::bigint BETWEEN 2000 AND 4000
             FROM pg_tviews_profile('post')), 'fan-out max near the 3000-post user');
SELECT must(warned('post', 'p99 fan-out through fk_user%', 100), 'W6 above the threshold');
SELECT must(NOT warned('post', 'p99 fan-out%', 100000), 'W6 below the threshold');

-- ========================================================================
-- W1 missing propagation index
-- ========================================================================
SELECT must(NOT warned('post', 'fk_user has no index%'), 'W1 with the index present');
DO $$ DECLARE ix TEXT; BEGIN
  SELECT c.relname INTO ix FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid
  JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
  WHERE i.indrelid = 'tv_post'::regclass AND a.attname = 'fk_user';
  EXECUTE format('DROP INDEX %I', ix);
END $$;
SELECT must(warned('post', 'fk_user has no index%'), 'W1 once the index is dropped');
SELECT count(*) FROM pg_tviews_ensure_propagation_indexes('post');
SELECT must(NOT warned('post', 'fk_user has no index%'), 'W1 after ensure_propagation_indexes');

-- ========================================================================
-- W2 low HOT ratio, W3 unused GIN, W4 fillfactor 100
-- ========================================================================
CREATE TABLE tb_doc (
    pk_doc BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id     UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    n      INT
);
INSERT INTO tb_doc (n) SELECT g FROM generate_series(1, 500) g;
SELECT tviews.pg_tviews_create('tv_doc', $$SELECT pk_doc, id, jsonb_build_object('n', n) AS data FROM tb_doc$$,
                                '{"data_gin_index": true, "fillfactor": 100}');
SELECT must(NOT warned('doc', 'HOT ratio%'), 'W2 before any update');
SELECT must(NOT warned('doc', 'fillfactor 100%'), 'W4 before any update');
UPDATE tb_doc SET n = n + 1;
UPDATE tb_doc SET n = n + 1;
UPDATE tb_doc SET n = n + 1;
SELECT settle();
SELECT must(warned('doc', 'HOT ratio%data_gin%'), 'W2 with a GIN index on data');
SELECT must(warned('doc', 'GIN index%never scanned%'), 'W3 for an unused GIN index');
SELECT must(warned('doc', 'fillfactor 100%'), 'W4 for fillfactor 100 with updates');
SELECT must(NOT warned('tag', 'fillfactor 100%') AND NOT warned('tag', 'GIN index%'),
            'W3/W4 on a default TVIEW');

-- ========================================================================
-- W5 TOAST share, W8 dead tuples
-- ========================================================================
CREATE TABLE tb_blob (
    pk_blob BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    payload TEXT
);
INSERT INTO tb_blob (payload)
SELECT (SELECT string_agg(md5(g::text || i::text), '') FROM generate_series(1, 200) i)
FROM generate_series(1, 200) g;
CREATE TABLE tv_blob AS SELECT pk_blob, id, jsonb_build_object('payload', payload) AS data FROM tb_blob;
SELECT must(warned('blob', '%of the table is TOAST%'), 'W5 for incompressible large documents');
SELECT must(NOT warned('tag', '%of the table is TOAST%'), 'W5 for small documents');

ALTER TABLE tv_tag SET (autovacuum_enabled = false);
ANALYZE tv_tag;
SELECT must(NOT warned('tag', '%dead tuples%'), 'W8 without dead tuples');
DELETE FROM tb_tag WHERE pk_tag <= 60;
SELECT settle();
SELECT must(warned('tag', '%dead tuples%'), 'W8 with 60% of the rows deleted');

\echo 'profile: PASS'
