-- Regression test for issue #156:
--   "Any UPDATE fails once a TVIEW has an extension-typed column"
--
-- The flush runs as the TVIEW's owner with search_path = pg_catalog, pg_temp
-- (#141). The refresh upsert guarded its write with a row-wise
-- IS DISTINCT FROM, which looks `=` up by name for each projected column type
-- when the statement is planned. An `=` installed outside pg_catalog (ltree,
-- citext, hstore, a domain over one of them) is not found, and the writer's
-- UPDATE fails with "operator does not exist". A type with no `=` at all
-- (json, point) failed the same way before #141, whatever the search_path.
--
-- Correct behaviour: the no-op guard compares record images (`*=`), which
-- needs no per-type operator. Every write refreshes the TVIEW, an unchanged
-- projection still writes nothing (#72), and equality is binary (decision D2):
-- citext 'A' -> 'a' counts as a change.
--
-- Left as is on purpose: `= ANY($n)` over bigint / uuid keys and jsonb
-- comparisons resolve to pg_catalog operators and are safe under the owner's
-- search_path.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_156_extension_typed_columns.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
-- Installed in public, as usual: their operators are not on the owner's search_path.
CREATE EXTENSION IF NOT EXISTS ltree;
CREATE EXTENSION IF NOT EXISTS citext;
CREATE EXTENSION IF NOT EXISTS hstore;

CREATE SCHEMA app;
CREATE TYPE app.mood AS ENUM ('sad', 'ok', 'happy');
CREATE DOMAIN app.label AS citext CHECK (VALUE <> '');

-- One table per column type, each projecting the column next to data.
CREATE TABLE tb_item (
    pk_item bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    label   text,
    path    ltree,
    name    citext,
    attrs   hstore,
    mood    app.mood,
    tag     app.label,
    doc     json,
    pos     point
);
INSERT INTO tb_item (label, path, name, attrs, mood, tag, doc, pos) VALUES
    ('a', 'a',   'A', 'k=>1', 'ok',    'X', '{"n": 1}', '(1,1)'),
    ('b', 'a.b', 'B', 'k=>2', 'happy', 'Y', '{"n": 2}', '(2,2)'),
    ('n', NULL,  NULL, NULL,  NULL,    NULL, NULL,      NULL);

SELECT pg_tviews_create('tv_item', $TV$
    SELECT pk_item, id, path, name, attrs, mood, tag, doc, pos,
           jsonb_build_object('id', id, 'label', label) AS data
    FROM tb_item
$TV$);

-- The TVIEW keeps the view's column types (they were stored as text before beta.21).
DO $$ BEGIN
    IF (SELECT format_type(atttypid, atttypmod) FROM pg_attribute
        WHERE attrelid = 'tv_item'::regclass AND attname = 'mood') <> 'app.mood' THEN
        RAISE EXCEPTION 'FAIL #156: tv_item.mood is %, view has app.mood',
            (SELECT format_type(atttypid, atttypmod) FROM pg_attribute
             WHERE attrelid = 'tv_item'::regclass AND attname = 'mood');
    END IF;
END $$;

-- A view that itself uses an extension operator: it is stored parsed, so the
-- owner's search_path does not matter for it.
CREATE TABLE tb_node (
    pk_node bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    path    ltree,
    label   text
);
INSERT INTO tb_node (path, label) VALUES ('a', 'root'), ('a.b', 'child'), ('z', 'other');
SELECT pg_tviews_create('tv_node', $TV$
    SELECT pk_node, id, path, jsonb_build_object('label', label) AS data
    FROM tb_node WHERE path <@ 'a'
$TV$);

CREATE FUNCTION _check(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM tv_item t FULL JOIN tviews.public__tv_item v USING (pk_item)
        WHERE t.data IS DISTINCT FROM v.data
           OR t.path::text IS DISTINCT FROM v.path::text
           OR t.name::text IS DISTINCT FROM v.name::text
           OR t.attrs::text IS DISTINCT FROM v.attrs::text
           OR t.mood::text IS DISTINCT FROM v.mood::text
           OR t.tag::text IS DISTINCT FROM v.tag::text
           OR t.doc::text IS DISTINCT FROM v.doc::text
           OR t.pos::text IS DISTINCT FROM v.pos::text)
    THEN
        RAISE EXCEPTION 'FAIL #156 [%]: tv_item diverges from tviews.public__tv_item', label;
    END IF;
    IF EXISTS (
        SELECT 1 FROM tv_node t FULL JOIN tviews.public__tv_node v USING (pk_node)
        WHERE t.data IS DISTINCT FROM v.data OR t.path::text IS DISTINCT FROM v.path::text)
    THEN
        RAISE EXCEPTION 'FAIL #156 [%]: tv_node diverges from tviews.public__tv_node', label;
    END IF;
END $$;

CREATE FUNCTION _updated_at(k bigint) RETURNS timestamptz LANGUAGE sql AS
    $$ SELECT updated_at FROM tv_item WHERE pk_item = k $$;
CREATE FUNCTION _skipped() RETURNS bigint LANGUAGE sql AS
    $$ SELECT (pg_tviews_queue_stats()->>'refresh_noop_skipped')::bigint $$;

-- ── 1. the issue's repro: UPDATE of a non-key column (upsert path) ──────────
SET pg_tviews.direct_patch_enabled = off;
INSERT INTO tb_item (label, path, name, attrs, mood, tag, doc, pos)
    VALUES ('c', 'a.c', 'C', 'k=>3', 'sad', 'Z', '{"n": 3}', '(3,3)');
SELECT _check('insert');
UPDATE tb_item SET label = 'b2' WHERE label = 'b';
SELECT _check('update label');
UPDATE tb_item SET path = 'a.b.x', attrs = 'k=>9', mood = 'sad', doc = '{"n": 9}', pos = '(9,9)'
    WHERE label = 'b2';
SELECT _check('update typed columns');
DELETE FROM tb_item WHERE label = 'c';
SELECT _check('delete');

-- ── 2. same on the direct-patch fast path ───────────────────────────────────
SET pg_tviews.direct_patch_enabled = on;
UPDATE tb_item SET label = 'a2', path = 'a.z' WHERE label = 'a';
SELECT _check('direct update');
INSERT INTO tb_item (label, path) VALUES ('d', 'a.d');
DELETE FROM tb_item WHERE label = 'd';
SELECT _check('direct insert/delete');
SET pg_tviews.direct_patch_enabled = off;

-- ── 3. #72 no-op guard still holds ──────────────────────────────────────────
CREATE TABLE _s AS SELECT _skipped() AS skipped, _updated_at(1) AS at1;
UPDATE tb_item SET label = label, path = path, name = name WHERE pk_item = 1;
DO $$ BEGIN
    IF _updated_at(1) IS DISTINCT FROM (SELECT at1 FROM _s) THEN
        RAISE EXCEPTION 'FAIL #156 [no-op]: an unchanged row was rewritten (updated_at moved)';
    END IF;
    IF _skipped() <= (SELECT skipped FROM _s) THEN
        RAISE EXCEPTION 'FAIL #156 [no-op]: refresh_noop_skipped did not grow';
    END IF;
END $$;

-- ── 4. NULL semantics: NULL vs NULL is equal, NULL vs value is a change ─────
UPDATE _s SET skipped = _skipped(), at1 = _updated_at(3);
UPDATE tb_item SET path = NULL, doc = NULL WHERE pk_item = 3;  -- already NULL
DO $$ BEGIN
    IF _updated_at(3) IS DISTINCT FROM (SELECT at1 FROM _s) THEN
        RAISE EXCEPTION 'FAIL #156 [NULL = NULL]: an all-NULL unchanged row was rewritten';
    END IF;
END $$;
UPDATE tb_item SET path = 'n' WHERE pk_item = 3;
DO $$ BEGIN
    IF _updated_at(3) IS NOT DISTINCT FROM (SELECT at1 FROM _s) THEN
        RAISE EXCEPTION 'FAIL #156 [NULL -> value]: the row was not rewritten';
    END IF;
END $$;
SELECT _check('null -> value');
UPDATE _s SET at1 = _updated_at(3);
UPDATE tb_item SET path = NULL WHERE pk_item = 3;
DO $$ BEGIN
    IF _updated_at(3) IS NOT DISTINCT FROM (SELECT at1 FROM _s) THEN
        RAISE EXCEPTION 'FAIL #156 [value -> NULL]: the row was not rewritten';
    END IF;
END $$;
SELECT _check('value -> null');

-- ── 5. D2: binary equality. citext 'A' -> 'a' rewrites the row ──────────────
UPDATE tb_item SET name = 'Q' WHERE pk_item = 1;
UPDATE _s SET at1 = _updated_at(1);
UPDATE tb_item SET name = 'q' WHERE pk_item = 1;
DO $$ BEGIN
    IF _updated_at(1) IS NOT DISTINCT FROM (SELECT at1 FROM _s) THEN
        RAISE EXCEPTION 'FAIL #156 [D2]: citext Q -> q did not rewrite the TVIEW row';
    END IF;
    IF (SELECT name::text FROM tv_item WHERE pk_item = 1) <> 'q' THEN
        RAISE EXCEPTION 'FAIL #156 [D2]: tv_item.name is %, expected q',
            (SELECT name::text FROM tv_item WHERE pk_item = 1);
    END IF;
END $$;

-- ── 6. the view with an extension operator keeps refreshing ─────────────────
UPDATE tb_node SET label = 'root2' WHERE path = 'a';
UPDATE tb_node SET path = 'a.z' WHERE label = 'other';
SELECT _check('view operator');

-- ── 7. create_or_replace reconciles the stored rows with the same guard ─────
SELECT pg_tviews_create_or_replace('tv_item', $TV$
    SELECT pk_item, id, path, name, attrs, mood, tag, doc, pos,
           jsonb_build_object('id', id, 'label', upper(label)) AS data
    FROM tb_item
$TV$);
SELECT _check('create_or_replace');
UPDATE tb_item SET path = 'a.r' WHERE pk_item = 1;
SELECT _check('after create_or_replace');

\echo 'PASS regress_issue_156_extension_typed_columns'
