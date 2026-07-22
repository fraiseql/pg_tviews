-- Regression test for issue #52:
--   "Refresh misses own-column changes when a TVIEW has a nested_object
--    dependency."
--
-- For a tview classified with a `nested_object` dependency, the smart-patch
-- UPDATE only patched the nested dependency path
-- (jsonb_smart_patch_nested(data, $1, ARRAY['<key>'])), so an UPDATE to a
-- column on the entity's OWN base table that is not part of the dependency
-- path was silently lost.
--
-- Correct behaviour: BOTH an own-column change (tb_post.title) AND a
-- dependency-path change (tb_user.name, projected as author.name) are
-- reflected in tv_post after refresh.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_52_nested_own_column.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_post CASCADE;
DROP VIEW  IF EXISTS v_post CASCADE;
DROP TABLE IF EXISTS tv_user CASCADE;
DROP VIEW  IF EXISTS v_user CASCADE;
DROP TABLE IF EXISTS tb_post CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;

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
INSERT INTO tb_user (name) VALUES ('Alice');
INSERT INTO tb_post (fk_user, title) VALUES (1, 'Original');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data
    FROM tb_user
$TVIEW$);

-- Unaliased `v_user.data` in the projection triggers nested_object classification.
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object('title', tb_post.title, 'author', v_user.data) AS data
    FROM tb_post LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
$TVIEW$);

-- Precondition: this test only exercises the bug if the dependency is nested.
DO $$ BEGIN
  IF NOT (SELECT dependency_types @> ARRAY['nested_object']::text[]
          FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION '#52 setup FAIL: tv_post is not classified nested_object (got %)',
      (SELECT dependency_types FROM pg_tview_meta WHERE entity = 'post');
  END IF;
END $$;

-- Baseline materialization.
DO $$ BEGIN
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) <> 'Original' THEN
    RAISE EXCEPTION '#52 setup FAIL: tv_post did not materialize title=Original';
  END IF;
  IF (SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1) <> 'Alice' THEN
    RAISE EXCEPTION '#52 setup FAIL: tv_post did not materialize author.name=Alice';
  END IF;
END $$;

-- (1) Own-column change on the entity's own base table must be reflected.
UPDATE tb_post SET title = 'CHANGED' WHERE pk_post = 1;
DO $$ BEGIN
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) <> 'CHANGED' THEN
    RAISE EXCEPTION '#52 FAIL: own-column change lost — title is % (expected CHANGED)',
      (SELECT data->>'title' FROM tv_post WHERE pk_post = 1);
  END IF;
  -- The nested dependency path must remain intact after an own-column patch.
  IF (SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1) <> 'Alice' THEN
    RAISE EXCEPTION '#52 FAIL: nested author path corrupted by own-column update (got %)',
      (SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1);
  END IF;
END $$;

-- (2) Dependency-path change (via the nested entity) must still be reflected.
UPDATE tb_user SET name = 'Bob' WHERE pk_user = 1;
DO $$ BEGIN
  IF (SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1) <> 'Bob' THEN
    RAISE EXCEPTION '#52 FAIL: nested dependency change lost — author.name is % (expected Bob)',
      (SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1);
  END IF;
  -- Own column must survive the dependency-path refresh.
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) <> 'CHANGED' THEN
    RAISE EXCEPTION '#52 FAIL: own column clobbered by dependency refresh (title=%)',
      (SELECT data->>'title' FROM tv_post WHERE pk_post = 1);
  END IF;
END $$;

SELECT '#52 PASS: nested_object tview reflects both own-column and dependency-path changes' AS result;
