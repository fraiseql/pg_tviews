-- Regression test for issue #56 (Phase 1): direct-patch column→key catalog map.
--
-- CREATE-time extraction records, per entity, which base-table columns map
-- identity-style to top-level keys of the entity's own `data` object. Bare base
-- columns are captured; nested objects / joined relations / expressions are not.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_catalog.sql

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
    name    TEXT,
    bio     TEXT
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER REFERENCES tb_user(pk_user),
    title   TEXT
);
INSERT INTO tb_user (name, bio) VALUES ('Alice', 'hello');
INSERT INTO tb_post (fk_user, title) VALUES (1, 'Original');

-- tv_user: two bare base columns map identity-style (name, bio).
SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);

-- tv_post: `title` is a bare (table-qualified) base column ⇒ mapped;
-- `author` embeds v_user.data (a joined relation) ⇒ NOT mapped.
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object('title', tb_post.title, 'author', v_user.data) AS data
    FROM tb_post LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
$TVIEW$);

-- (1) tv_user map: exactly {name→name, bio→bio}, order-independent.
DO $$
DECLARE cols text[]; keys text[];
BEGIN
  SELECT direct_map_columns, direct_map_keys INTO cols, keys
  FROM pg_tview_meta WHERE entity = 'user';

  IF NOT (cols @> ARRAY['name','bio']::text[] AND cols <@ ARRAY['name','bio']::text[]) THEN
    RAISE EXCEPTION '#56 FAIL: user direct_map_columns is % (expected name,bio)', cols;
  END IF;
  IF NOT (keys @> ARRAY['name','bio']::text[] AND keys <@ ARRAY['name','bio']::text[]) THEN
    RAISE EXCEPTION '#56 FAIL: user direct_map_keys is % (expected name,bio)', keys;
  END IF;
  -- Aligned by index: the key for column `bio` must be `bio`.
  IF keys[array_position(cols, 'bio')] <> 'bio' THEN
    RAISE EXCEPTION '#56 FAIL: user map misaligned (bio → %)', keys[array_position(cols, 'bio')];
  END IF;
END $$;

-- (2) tv_post map: contains `title` (bare base col), NOT `author` / not the
--     joined `v_user.data`.
DO $$
DECLARE cols text[]; keys text[];
BEGIN
  SELECT direct_map_columns, direct_map_keys INTO cols, keys
  FROM pg_tview_meta WHERE entity = 'post';

  IF NOT (cols @> ARRAY['title']::text[]) THEN
    RAISE EXCEPTION '#56 FAIL: post direct_map_columns missing title (got %)', cols;
  END IF;
  IF cols @> ARRAY['author']::text[] OR keys @> ARRAY['author']::text[] THEN
    RAISE EXCEPTION '#56 FAIL: post map wrongly captured the nested author key (cols=%, keys=%)', cols, keys;
  END IF;
  IF keys[array_position(cols, 'title')] <> 'title' THEN
    RAISE EXCEPTION '#56 FAIL: post map misaligned for title (got %)',
      keys[array_position(cols, 'title')];
  END IF;
END $$;

-- (3) Column/key arrays are always aligned in length for every tview.
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM pg_tview_meta
             WHERE cardinality(direct_map_columns) <> cardinality(direct_map_keys)) THEN
    RAISE EXCEPTION '#56 FAIL: direct_map_columns / direct_map_keys length mismatch';
  END IF;
END $$;

SELECT 'issue #56 catalog map: PASS' AS result;
