-- Hardening: column-aware refresh in a NON-public schema. `view_source_columns`
-- resolves the backing view v_<entity> and its source table by schema; if the
-- schema were threaded wrong the source-column set would come back empty (safe,
-- but the skip would silently never fire). This proves the skip AND the cascade
-- both work under a named search_path — the benchmark's environment.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_column_aware_schema.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP SCHEMA IF EXISTS app CASCADE;
CREATE SCHEMA app;
SET search_path TO app, public;

CREATE TABLE app.tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, username TEXT, bio TEXT
);
CREATE TABLE app.tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER REFERENCES app.tb_user, title TEXT
);

INSERT INTO app.tb_user (username, bio) VALUES ('alice', 'a-bio'), ('zoe', 'z-bio');
INSERT INTO app.tb_post (fk_author, title) VALUES (1, 'P1'), (1, 'P2'), (1, 'P3');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('username', username, 'bio', bio) AS data
    FROM app.tb_user
$TVIEW$);
-- Lean: embeds username only (NOT bio), joined to the app-schema base table.
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT p.pk_post, p.id, p.fk_author, u.id AS author_id,
           jsonb_build_object('title', p.title,
               'author', jsonb_build_object('username', u.username)) AS data
    FROM app.tb_post p JOIN app.tb_user u ON u.pk_user = p.fk_author
$TVIEW$);

-- The tviews live in the app schema.
DO $$ BEGIN
  IF to_regclass('app.tv_post') IS NULL THEN
    RAISE EXCEPTION 'setup FAIL: app.tv_post not created in the app schema';
  END IF;
END $$;

CREATE FUNCTION app._rc() RETURNS bigint LANGUAGE sql AS
  $$ SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint $$;

-- ── Non-projected column (bio) in a named schema → cascade SKIPPED ───────────
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := app._rc();
  UPDATE app.tb_user SET bio = 'a-bio-CHANGED' WHERE pk_user = 1;
  IF app._rc() <> rc0 THEN
    RAISE EXCEPTION 'schema FAIL: bio update triggered % recompute(s) — schema threading likely dropped source_columns',
      app._rc() - rc0;
  END IF;
  IF (SELECT data->>'bio' FROM app.tv_user WHERE pk_user = 1) <> 'a-bio-CHANGED' THEN
    RAISE EXCEPTION 'schema FAIL: app.tv_user.bio not refreshed';
  END IF;
END $$;

-- ── Projected column (username) in a named schema → cascade RUNS ─────────────
DO $$ BEGIN
  UPDATE app.tb_user SET username = 'alice2' WHERE pk_user = 1;
  IF (SELECT count(*) FROM app.tv_post WHERE data->'author'->>'username' = 'alice2') <> 3 THEN
    RAISE EXCEPTION 'schema FAIL: username change did not cascade in the app schema (% of 3)',
      (SELECT count(*) FROM app.tv_post WHERE data->'author'->>'username' = 'alice2');
  END IF;
END $$;

RESET search_path;
SELECT 'column-aware schema: PASS' AS result;
