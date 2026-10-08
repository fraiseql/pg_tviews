-- Regression test for issue #71:
--   "Cascade propagation seq-scans parent TVIEWs: integer fk_* columns get no index"
--
-- Cascade propagation finds the parent rows to refresh with
--   SELECT fk_<child>, pk_<parent> FROM tv_<parent> WHERE fk_<child> = ANY($1)
-- but TVIEW creation indexed only id, UUID FKs and data, so every cascade step
-- was a sequential scan of the whole parent TVIEW.
--
-- Correct behaviour: every integer fk_* column of a TVIEW gets a btree on
-- (fk_<x>, pk_<entity>), on both creation paths (pg_tviews_create and
-- CREATE TABLE tv_* AS SELECT), and the propagation query uses it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_71_fk_propagation_index.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_comment, tv_post, tv_user CASCADE;
DROP VIEW  IF EXISTS tviews.public__tv_comment, tviews.public__tv_post, tviews.public__tv_user CASCADE;
DROP TABLE IF EXISTS tb_comment, tb_post, tb_user CASCADE;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER REFERENCES tb_user(pk_user),
    title   TEXT
);
CREATE TABLE tb_comment (
    pk_comment INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_post    INTEGER REFERENCES tb_post(pk_post),
    fk_user    INTEGER REFERENCES tb_user(pk_user),
    body       TEXT
);
INSERT INTO tb_user (name) SELECT 'user ' || g FROM generate_series(1, 500) g;
INSERT INTO tb_post (fk_user, title) SELECT 1 + g % 500, 'post ' || g FROM generate_series(1, 5000) g;
INSERT INTO tb_comment (fk_post, fk_user, body)
SELECT 1 + g % 5000, 1 + g % 500, 'comment ' || g FROM generate_series(1, 5000) g;

-- True when an index on `tbl` leads with (fk, pk).
CREATE FUNCTION pg_temp.has_fk_pk_index(tbl regclass, fk text, pk text) RETURNS boolean
LANGUAGE sql AS $$
    SELECT EXISTS (
        SELECT 1 FROM pg_index i
        WHERE i.indrelid = tbl
          AND i.indnkeyatts >= 2
          AND (SELECT attname FROM pg_attribute WHERE attrelid = tbl AND attnum = i.indkey[0]) = fk
          AND (SELECT attname FROM pg_attribute WHERE attrelid = tbl AND attnum = i.indkey[1]) = pk)
$$;

-- Top node of the propagation lookup's plan with seq scans disabled: a table
-- without a usable index still gets a (disabled) Seq Scan, so this is deterministic.
CREATE FUNCTION pg_temp.propagation_plan(tbl text, fk text, pk text) RETURNS text
LANGUAGE plpgsql AS $$
DECLARE plan json;
BEGIN
    SET LOCAL enable_seqscan = off;
    EXECUTE format('EXPLAIN (FORMAT JSON) SELECT %I, %I FROM %I WHERE %I = ANY(%L::bigint[])',
                   fk, pk, tbl, fk, '{1,2}')
       INTO plan;
    RETURN plan -> 0 -> 'Plan' ->> 'Node Type';
END $$;

-- ── Path 1: pg_tviews_create ────────────────────────────────────────────────
SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$TVIEW$);
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object('title', tb_post.title, 'author', tv_user.data) AS data
    FROM tb_post LEFT JOIN tv_user ON tv_user.pk_user = tb_post.fk_user
$TVIEW$);
ANALYZE tv_post;

DO $$ BEGIN
    IF NOT pg_temp.has_fk_pk_index('tv_post', 'fk_user', 'pk_post') THEN
        RAISE EXCEPTION 'FAIL #71: tv_post has no (fk_user, pk_post) index';
    END IF;
    IF pg_temp.propagation_plan('tv_post', 'fk_user', 'pk_post') = 'Seq Scan' THEN
        RAISE EXCEPTION 'FAIL #71: propagation lookup on tv_post is a Seq Scan';
    END IF;
END $$;

-- ── Path 2: CREATE TABLE tv_* AS SELECT (two integer FKs) ────────────────────
CREATE TABLE tv_comment AS
    SELECT tb_comment.pk_comment, tb_comment.id, tb_comment.fk_post, tb_comment.fk_user,
           jsonb_build_object('body', tb_comment.body, 'post', tv_post.data) AS data
    FROM tb_comment LEFT JOIN tv_post ON tv_post.pk_post = tb_comment.fk_post;
ANALYZE tv_comment;

DO $$ BEGIN
    IF NOT pg_temp.has_fk_pk_index('tv_comment', 'fk_post', 'pk_comment') THEN
        RAISE EXCEPTION 'FAIL #71: tv_comment (CTAS) has no (fk_post, pk_comment) index';
    END IF;
    IF NOT pg_temp.has_fk_pk_index('tv_comment', 'fk_user', 'pk_comment') THEN
        RAISE EXCEPTION 'FAIL #71: tv_comment (CTAS) has no (fk_user, pk_comment) index';
    END IF;
    IF pg_temp.propagation_plan('tv_comment', 'fk_post', 'pk_comment') = 'Seq Scan' THEN
        RAISE EXCEPTION 'FAIL #71: propagation lookup on tv_comment is a Seq Scan';
    END IF;
END $$;

-- ── The cascade still produces the right rows ───────────────────────────────
UPDATE tb_user SET name = 'renamed' WHERE pk_user = 7;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post)
               WHERE t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'FAIL #71: tv_post diverges from tviews.public__tv_post after cascade';
    END IF;
    IF EXISTS (SELECT 1 FROM tv_comment t JOIN tviews.public__tv_comment v USING (pk_comment)
               WHERE t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'FAIL #71: tv_comment diverges from tviews.public__tv_comment after cascade';
    END IF;
END $$;

-- ── Existing installs: pg_tviews_ensure_propagation_indexes() ───────────────
-- Simulate TVIEWs created before this fix by dropping their propagation indexes.
DO $$
DECLARE r record;
BEGIN
    FOR r IN SELECT i.indexrelid::regclass AS idx
             FROM pg_index i
             WHERE i.indrelid IN ('tv_post'::regclass, 'tv_comment'::regclass)
               AND i.indnkeyatts = 2 AND NOT i.indisprimary
    LOOP
        EXECUTE format('DROP INDEX %s', r.idx);
    END LOOP;
END $$;

DO $$
DECLARE n int;
BEGIN
    IF pg_temp.has_fk_pk_index('tv_post', 'fk_user', 'pk_post') THEN
        RAISE EXCEPTION 'setup: tv_post propagation index was not dropped';
    END IF;

    -- dry_run reports the DDL and creates nothing: tv_post.fk_user and
    -- tv_comment.fk_post hold an embedded TVIEW's key; tv_comment.fk_user holds
    -- none, so no lookup goes through it.
    SELECT count(*) INTO n FROM pg_tviews_ensure_propagation_indexes(NULL, true);
    IF n <> 2 THEN
        RAISE EXCEPTION 'FAIL #71: dry run reported % statements, expected 2', n;
    END IF;
    IF pg_temp.has_fk_pk_index('tv_post', 'fk_user', 'pk_post') THEN
        RAISE EXCEPTION 'FAIL #71: dry run created an index';
    END IF;

    -- entity filter: only tv_post
    SELECT count(*) INTO n FROM pg_tviews_ensure_propagation_indexes('post');
    IF n <> 1 OR NOT pg_temp.has_fk_pk_index('tv_post', 'fk_user', 'pk_post') THEN
        RAISE EXCEPTION 'FAIL #71: ensure(''post'') created % indexes, expected tv_post''s 1', n;
    END IF;
    IF pg_temp.has_fk_pk_index('tv_comment', 'fk_post', 'pk_comment') THEN
        RAISE EXCEPTION 'FAIL #71: ensure(''post'') touched tv_comment';
    END IF;

    -- a user index that already leads with the fk counts as covering it
    CREATE INDEX user_owned_fk_user ON tv_comment (fk_user);

    SELECT count(*) INTO n FROM pg_tviews_ensure_propagation_indexes();
    IF n <> 1 OR NOT pg_temp.has_fk_pk_index('tv_comment', 'fk_post', 'pk_comment') THEN
        RAISE EXCEPTION 'FAIL #71: ensure() created % indexes, expected tv_comment.fk_post only', n;
    END IF;
    IF pg_temp.has_fk_pk_index('tv_comment', 'fk_user', 'pk_comment') THEN
        RAISE EXCEPTION 'FAIL #71: ensure() duplicated the user-owned fk_user index';
    END IF;

    -- idempotent
    SELECT count(*) INTO n FROM pg_tviews_ensure_propagation_indexes();
    IF n <> 0 THEN
        RAISE EXCEPTION 'FAIL #71: second ensure() call created % indexes, expected 0', n;
    END IF;
END $$;

\echo 'PASS regress_issue_71_fk_propagation_index'
