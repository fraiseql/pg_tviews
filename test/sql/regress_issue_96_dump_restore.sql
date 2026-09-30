-- Regression test for issue #96:
--   "pg_restore of a TVIEW: pg_tview_meta comes back empty, so the restored
--    TVIEW silently stops propagating."
--
-- pg_tview_meta and pg_tview_helpers are extension-owned. Without
-- pg_extension_config_dump their rows are never dumped, and even if they were,
-- raw OIDs would point at nothing in the restored database.
--
-- Correct behaviour: pg_dump -Fc followed by pg_restore into a fresh database
-- gives back registered TVIEWs whose catalog rows name the restored relations,
-- and which keep propagating (direct writes and cascades). A CTAS under an
-- empty search_path (what restore-style scripts run with) is still converted.
--
-- The round trip shells out to pg_dump/pg_restore (psql \!), honouring
-- PGHOST/PGPORT/PGUSER, and restores into <current db>_restored.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_96_dump_restore.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- ========================================================================
-- Source database: two TVIEWs, one embedding the other, one in its own schema
-- ========================================================================
CREATE SCHEMA app;

CREATE TABLE tb_author (
    pk_author INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name      TEXT
);
CREATE TABLE app.tb_post (
    pk_post   INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER NOT NULL REFERENCES tb_author(pk_author),
    title     TEXT
);
INSERT INTO tb_author (name) VALUES ('ann'), ('bob');
INSERT INTO app.tb_post (fk_author, title) VALUES (1, 'p1'), (2, 'p2');

CREATE TABLE tv_author AS
SELECT pk_author, id, jsonb_build_object('id', id, 'name', name) AS data
FROM tb_author;

CREATE TABLE app.tv_post AS
SELECT p.pk_post, p.id, p.fk_author,
       jsonb_build_object('id', p.id, 'title', p.title,
                          'author_name', a.name) AS data
FROM app.tb_post p
JOIN public.tb_author a ON a.pk_author = p.fk_author;

DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post'
                   AND cardinality(cascade_paths) > 0) THEN
    RAISE EXCEPTION '#96 setup FAIL: tv_post has no cascade path from tb_author';
  END IF;
  IF (SELECT count(*) FROM pg_tview_meta) <> 2 THEN
    RAISE EXCEPTION '#96 setup FAIL: expected 2 registered TVIEWs, got %',
      (SELECT count(*) FROM pg_tview_meta);
  END IF;
END $$;

-- ========================================================================
-- Round trip: pg_dump -Fc | pg_restore --exit-on-error into a fresh database
-- ========================================================================
\setenv PGTV_SRC :DBNAME
\! dropdb --if-exists "${PGTV_SRC}_restored" && createdb "${PGTV_SRC}_restored" && pg_dump -Fc -f "/tmp/${PGTV_SRC}.dump" "$PGTV_SRC" && pg_restore --exit-on-error -d "${PGTV_SRC}_restored" "/tmp/${PGTV_SRC}.dump"
\if :SHELL_ERROR
  \echo '#96 FAIL: pg_dump / pg_restore round trip exited with' :SHELL_EXIT_CODE
  \quit 3
\endif

\set restored :DBNAME '_restored'
\c :restored
SET client_min_messages TO WARNING;

-- ========================================================================
-- Cycle 1: the catalog comes back and names the restored relations
-- ========================================================================
DO $$ BEGIN
  IF (SELECT count(*) FROM pg_tview_meta) <> 2 THEN
    RAISE EXCEPTION '#96 FAIL: pg_tview_meta has % rows after restore, expected 2',
      (SELECT count(*) FROM pg_tview_meta);
  END IF;
  IF (SELECT table_oid::oid FROM pg_tview_meta WHERE entity = 'author')
       IS DISTINCT FROM 'public.tv_author'::regclass::oid
     OR (SELECT view_oid::oid FROM pg_tview_meta WHERE entity = 'author')
       IS DISTINCT FROM 'public.v_author'::regclass::oid THEN
    RAISE EXCEPTION '#96 FAIL: author catalog row does not point at the restored tv_author / v_author';
  END IF;
  IF (SELECT table_oid::oid FROM pg_tview_meta WHERE entity = 'post')
       IS DISTINCT FROM 'app.tv_post'::regclass::oid
     OR (SELECT view_oid::oid FROM pg_tview_meta WHERE entity = 'post')
       IS DISTINCT FROM 'app.v_post'::regclass::oid THEN
    RAISE EXCEPTION '#96 FAIL: post catalog row does not point at the restored app.tv_post / app.v_post';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta m, unnest(m.cascade_paths) cp
             WHERE m.entity = 'post'
               AND (cp::jsonb->>'source_oid')::oid <> 'public.tb_author'::regclass::oid) THEN
    RAISE EXCEPTION '#96 FAIL: post cascade path still carries the source database''s tb_author OID';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta m, unnest(m.cascade_paths) cp,
                    jsonb_array_elements(cp::jsonb->'hops') h
             WHERE m.entity = 'post'
               AND (h->>'table_oid')::oid <> 'app.tb_post'::regclass::oid) THEN
    RAISE EXCEPTION '#96 FAIL: post cascade hop still carries the source database''s tb_post OID';
  END IF;
  IF (SELECT count(*) FROM tv_author) <> 2 OR (SELECT count(*) FROM app.tv_post) <> 2 THEN
    RAISE EXCEPTION '#96 FAIL: restored TVIEW rows missing';
  END IF;
END $$;

-- ========================================================================
-- Cycle 2: the restored TVIEWs keep propagating (insert + cascade)
-- ========================================================================
-- Refresh resolves a TVIEW's backing view through the search_path.
SET search_path = public, app;
INSERT INTO tb_author (name) VALUES ('cid');
INSERT INTO app.tb_post (fk_author, title) VALUES (3, 'p3');
UPDATE tb_author SET name = 'ann2' WHERE pk_author = 1;

DO $$ BEGIN
  IF (SELECT count(*) FROM tv_author) <> 3 THEN
    RAISE EXCEPTION '#96 FAIL: INSERT into tb_author did not reach tv_author after restore';
  END IF;
  IF (SELECT data->>'title' FROM app.tv_post WHERE pk_post = 3) IS DISTINCT FROM 'p3' THEN
    RAISE EXCEPTION '#96 FAIL: INSERT into app.tb_post did not reach app.tv_post after restore';
  END IF;
  IF (SELECT data->>'author_name' FROM app.tv_post WHERE pk_post = 1) IS DISTINCT FROM 'ann2' THEN
    RAISE EXCEPTION '#96 FAIL: author rename did not cascade into app.tv_post after restore (got %)',
      (SELECT data->>'author_name' FROM app.tv_post WHERE pk_post = 1);
  END IF;
END $$;

-- ========================================================================
-- Cycle 3: a CTAS under an empty search_path (restore-style) is converted
-- ========================================================================
CREATE TABLE public.tb_tag (
    pk_tag INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id     UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    label  TEXT
);
INSERT INTO public.tb_tag (label) VALUES ('t1');

SET search_path = '';
CREATE TABLE public.tv_tag AS
SELECT pk_tag, id, pg_catalog.jsonb_build_object('label', label) AS data
FROM public.tb_tag;
RESET search_path;

DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'tag') THEN
    RAISE EXCEPTION '#96 FAIL: CTAS tv_tag under search_path = '''' was not registered';
  END IF;
END $$;

-- ========================================================================
-- Cleanup
-- ========================================================================
\c postgres
\! dropdb --if-exists "${PGTV_SRC}_restored"; rm -f "/tmp/${PGTV_SRC}.dump"
