-- Regression test for issue #75:
--   "UNLOGGED TVIEWs (the default) are unreadable on hot standbys; empty after
--    promotion."
--
-- PostgreSQL refuses to read UNLOGGED relations during recovery and resets them
-- to their (empty) init fork on promotion or after a crash. pg_tviews repopulated
-- such a TVIEW only lazily, on the first write that touched it.
--
-- Correct behaviour: clients can tell which TVIEWs a standby can serve
-- (pg_tviews_is_replica_readable / pg_tviews_replication_status), deploy tooling
-- can rebuild every emptied TVIEW at once (pg_tviews_rebuild_all), and a TVIEW
-- can be switched to LOGGED (pg_tviews_set_logged). TRUNCATE stands in for the
-- init-fork reset here; test/replication/promote_rebuild.sh runs a real standby.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_75_replication_status.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    title   TEXT
);
CREATE TABLE tb_tag (
    pk_tag BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id     UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    label  TEXT
);
INSERT INTO tb_user (name) VALUES ('ann'), ('bob');
INSERT INTO tb_post (fk_user, title) VALUES (1, 'p1'), (2, 'p2'), (2, 'p3');
INSERT INTO tb_tag (label) VALUES ('t1');

-- Two UNLOGGED TVIEWs (the default), post embedding user, and one LOGGED.
CREATE TABLE tv_user AS
SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user;
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_user,
       jsonb_build_object('title', p.title, 'user', v_user.data) AS data
FROM tb_post p JOIN v_user ON v_user.pk_user = p.fk_user;
BEGIN;
SET LOCAL pg_tviews.unlogged_by_default = off;
CREATE TABLE tv_tag AS
SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM tb_tag;
COMMIT;

-- ========================================================================
-- Cycle 1: detection
-- ========================================================================
DO $$ BEGIN
  IF pg_tviews_is_replica_readable('user') OR NOT pg_tviews_is_replica_readable('tag') THEN
    RAISE EXCEPTION '#75 FAIL: is_replica_readable user=% tag=%',
      pg_tviews_is_replica_readable('user'), pg_tviews_is_replica_readable('tag');
  END IF;
  IF pg_tviews_is_replica_readable('nope') IS NOT NULL THEN
    RAISE EXCEPTION '#75 FAIL: is_replica_readable of an unknown entity should be NULL';
  END IF;
  IF (SELECT array_agg(entity || ':' || persistence || ':' || replica_readable
                       || ':' || is_empty || ':' || needs_rebuild ORDER BY entity)
      FROM pg_tviews_replication_status())
     IS DISTINCT FROM ARRAY['post:unlogged:false:false:false',
                            'tag:logged:true:false:false',
                            'user:unlogged:false:false:false'] THEN
    RAISE EXCEPTION '#75 FAIL: replication_status %',
      (SELECT array_agg(s ORDER BY s.entity) FROM pg_tviews_replication_status() s);
  END IF;
END $$;

-- ========================================================================
-- Cycle 2: rebuild_all repopulates emptied TVIEWs, dependencies first
-- ========================================================================
TRUNCATE tv_user, tv_post;

DO $$ BEGIN
  IF (SELECT array_agg(entity ORDER BY entity) FROM pg_tviews_replication_status()
      WHERE needs_rebuild) IS DISTINCT FROM ARRAY['post', 'user'] THEN
    RAISE EXCEPTION '#75 FAIL: needs_rebuild after reset %',
      (SELECT array_agg(s ORDER BY s.entity) FROM pg_tviews_replication_status() s);
  END IF;
END $$;

CREATE TEMP TABLE rebuilt AS
SELECT row_number() OVER () AS step, * FROM pg_tviews_rebuild_all();

DO $$ BEGIN
  IF (SELECT array_agg(entity || ':' || rows ORDER BY step) FROM rebuilt)
     IS DISTINCT FROM ARRAY['user:2', 'post:3'] THEN
    RAISE EXCEPTION '#75 FAIL: rebuild_all returned %',
      (SELECT array_agg(entity || ':' || rows ORDER BY step) FROM rebuilt);
  END IF;
  IF (SELECT count(*) FROM tv_post) <> 3
     OR (SELECT data->'user'->>'name' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'ann' THEN
    RAISE EXCEPTION '#75 FAIL: tv_post not repopulated';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tviews_rebuild_all()) THEN
    RAISE EXCEPTION '#75 FAIL: a second rebuild_all() rebuilt a healthy TVIEW';
  END IF;
  IF (SELECT count(*) FROM pg_tviews_rebuild_all(only_empty => false)) <> 3 THEN
    RAISE EXCEPTION '#75 FAIL: rebuild_all(only_empty => false) did not rebuild every TVIEW';
  END IF;
END $$;

-- The rebuilt TVIEWs keep propagating.
UPDATE tb_user SET name = 'ann2' WHERE pk_user = 1;
DO $$ BEGIN
  IF (SELECT data->'user'->>'name' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'ann2' THEN
    RAISE EXCEPTION '#75 FAIL: cascade after rebuild_all';
  END IF;
END $$;

-- ========================================================================
-- Cycle 3: set_logged switches persistence (and back)
-- ========================================================================
SELECT pg_tviews_set_logged('user', true);
DO $$ BEGIN
  IF NOT pg_tviews_is_replica_readable('user') OR (SELECT count(*) FROM tv_user) <> 2 THEN
    RAISE EXCEPTION '#75 FAIL: set_logged(user, true)';
  END IF;
END $$;
SELECT pg_tviews_set_logged('user', false);
DO $$ BEGIN
  IF pg_tviews_is_replica_readable('user') THEN
    RAISE EXCEPTION '#75 FAIL: set_logged(user, false)';
  END IF;
END $$;
