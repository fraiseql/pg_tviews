-- Regression test for issue #182: a hierarchy stored as a path of ids, joined to
-- the ancestors three ways (a target-list unnest in a subquery, a LATERAL unnest,
-- `= ANY (<array>)`). Each spelling must classify the same: the TVIEW's own read of
-- its tb_<entity> is local, the ancestor read is mapped through the array
-- membership, so a soft delete or a rename refreshes every row it changes. A join on a
-- computed output of a subquery (`upper(n.name) AS upper_name`) is mapped too.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_182_array_joins.sql
--
-- expect-output: map to tv_area keys with a sequential scan of tb_area
-- expect-once: CREATE INDEX ON public.tb_area USING gin
-- expect-output: issue #182 array joins: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

-- The controls below are all_keys on purpose: the policy is declared here.
SET pg_tviews.uncascaded_policy = 'warn';

-- One copy of the node tree per spelling (tb_<entity>, as the issue's tb_node).
DO $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY ARRAY['nsrf', 'nlat', 'nany', 'nbadge'] LOOP
        EXECUTE format('CREATE TABLE tb_%1$s (pk_%1$s bigint PRIMARY KEY,
                            id uuid UNIQUE NOT NULL DEFAULT gen_random_uuid(),
                            path text NOT NULL, name text, deleted_at timestamptz)', e);
        EXECUTE format($q$INSERT INTO tb_%1$s (pk_%1$s, path, name)
                        VALUES (1, '1', 'root'), (2, '1.2', 'child'), (3, '1.2.3', 'leaf'), (4, '1.4', 'other')$q$, e);
    END LOOP;
END $$;
CREATE TABLE tb_badge (pk_badge bigint PRIMARY KEY, code text NOT NULL, label text);
INSERT INTO tb_badge VALUES (1, 'ROOT', 'r'), (2, 'CHILD', 'c');

-- Write the same change to every copy of the tree.
CREATE FUNCTION write_all(stmt text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY ARRAY['nsrf', 'nlat', 'nany', 'nbadge'] LOOP
        EXECUTE replace(replace(stmt, 'tb_x', 'tb_' || e), 'pk_x', 'pk_' || e);
    END LOOP;
END $$;

-- ── three spellings of the same view ────────────────────────────────────────
SELECT pg_tviews_create('tv_nsrf', $$
    SELECT s.pk_nsrf, s.id,
           jsonb_build_object('name', s.name, 'path_of_names', array_agg(a.name ORDER BY a.pk_nsrf)) AS data
    FROM (SELECT n.pk_nsrf, n.id, n.name, unnest(string_to_array(n.path, '.')::bigint[]) AS node_id
          FROM tb_nsrf n WHERE n.deleted_at IS NULL) s
    JOIN tb_nsrf a ON a.pk_nsrf = s.node_id AND a.deleted_at IS NULL
    GROUP BY s.pk_nsrf, s.id, s.name $$);
SELECT pg_tviews_create('tv_nlat', $$
    SELECT n.pk_nlat, n.id,
           jsonb_build_object('name', n.name, 'path_of_names', array_agg(a.name ORDER BY a.pk_nlat)) AS data
    FROM tb_nlat n
    CROSS JOIN LATERAL unnest(string_to_array(n.path, '.')::bigint[]) AS u(node_id)
    JOIN tb_nlat a ON a.pk_nlat = u.node_id AND a.deleted_at IS NULL
    WHERE n.deleted_at IS NULL
    GROUP BY n.pk_nlat, n.id, n.name $$);
SELECT pg_tviews_create('tv_nany', $$
    SELECT n.pk_nany, n.id,
           jsonb_build_object('name', n.name, 'path_of_names', array_agg(a.name ORDER BY a.pk_nany)) AS data
    FROM tb_nany n
    JOIN tb_nany a ON a.pk_nany = ANY (string_to_array(n.path, '.')::bigint[]) AND a.deleted_at IS NULL
    WHERE n.deleted_at IS NULL
    GROUP BY n.pk_nany, n.id, n.name $$);
-- A join on a computed output of a subquery.
SELECT pg_tviews_create('tv_nbadge', $$
    SELECT s.pk_nbadge, s.id, jsonb_build_object('name', s.name, 'badge', x.label) AS data
    FROM (SELECT n.pk_nbadge, n.id, n.name, upper(n.name) AS upper_name
          FROM tb_nbadge n WHERE n.deleted_at IS NULL) s
    LEFT JOIN tb_badge x ON x.code = s.upper_name $$);

CREATE FUNCTION check_all(label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY ARRAY['nsrf', 'nlat', 'nany', 'nbadge'] LOOP
        IF fresh_diff(('tv_' || e)::regclass, 'pk_' || e) IS NOT NULL THEN
            RAISE EXCEPTION '#182 FAIL: % after %', fresh_diff(('tv_' || e)::regclass, 'pk_' || e), label;
        END IF;
    END LOOP;
END $$;

SELECT write_all('UPDATE tb_x SET deleted_at = now() WHERE pk_x = 3');
SELECT check_all('the soft delete of node 3');
SELECT write_all($$UPDATE tb_x SET name = 'renamed' WHERE pk_x = 2$$);
SELECT check_all('the rename of node 2 (an ancestor of 3)');
SELECT write_all('UPDATE tb_x SET deleted_at = NULL WHERE pk_x = 3');
SELECT check_all('the restore of node 3');
SELECT write_all($$UPDATE tb_x SET path = '1.4.3' WHERE pk_x = 3$$);
SELECT check_all('the move of node 3 under node 4');
SELECT write_all($$UPDATE tb_x SET name = 'Child' WHERE pk_x = 4$$);
SELECT check_all('the rename of node 4 to a badge code');
UPDATE tb_badge SET label = 'c2' WHERE pk_badge = 2;
SELECT check_all('a badge label update');
INSERT INTO tb_badge VALUES (3, 'LEAF', 'l');
SELECT check_all('a badge insert');
DELETE FROM tb_badge WHERE pk_badge = 1;
SELECT check_all('a badge delete');
SELECT write_all($$INSERT INTO tb_x (pk_x, path, name) VALUES (5, '1.4.5', 'new')$$);
SELECT check_all('a node insert');
SELECT write_all('DELETE FROM tb_x WHERE pk_x = 4');
SELECT check_all('the delete of node 4');

-- ── classification: the same for the three spellings ────────────────────────
DO $$
DECLARE r record;
BEGIN
    FOR r IN SELECT entity, cascade_kinds, uncascaded_tables FROM tviews.registry
             WHERE entity IN ('nsrf', 'nlat', 'nany') LOOP
        IF r.cascade_kinds <> jsonb_build_object('tb_' || r.entity, 'mapped')
           OR cardinality(r.uncascaded_tables) <> 0 THEN
            RAISE EXCEPTION '#182 FAIL: tv_% classifies % (uncascaded: %)',
                r.entity, r.cascade_kinds, r.uncascaded_tables;
        END IF;
    END LOOP;
    IF (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'nbadge')
       <> '{"tb_nbadge": "local", "tb_badge": "mapped"}' THEN
        RAISE EXCEPTION '#182 FAIL: the computed join classifies %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'nbadge');
    END IF;
    -- The ancestor read maps through the array membership.
    IF tviews.pg_tviews_mapping_query('tv_nsrf', 'tb_nsrf'::regclass) NOT LIKE '%ANY%' THEN
        RAISE EXCEPTION '#182 FAIL: the ancestor mapping of tv_nsrf is %',
            tviews.pg_tviews_mapping_query('tv_nsrf', 'tb_nsrf'::regclass);
    END IF;
END $$;

-- ── controls ────────────────────────────────────────────────────────────────
CREATE TABLE tb_nvol (pk_nvol bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_ntop (pk_ntop bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_nvol (pk_nvol, name) VALUES (1, 'root'), (2, 'child');
INSERT INTO tb_ntop (pk_ntop, name) VALUES (1, 'root'), (2, 'child');
-- A computed output that is not immutable links nothing.
SELECT pg_tviews_create('tv_nvol', $$
    SELECT s.pk_nvol, s.id, jsonb_build_object('badge', x.label) AS data
    FROM (SELECT n.pk_nvol, n.id, upper(n.name) || to_char(now(), '') AS upper_name FROM tb_nvol n) s
    LEFT JOIN tb_badge x ON x.code = s.upper_name $$);
-- A set-returning function in the top-level SELECT keeps the TVIEW without a root.
SELECT pg_tviews_create('tv_ntop', $$
    SELECT n.pk_ntop, n.id, jsonb_build_object('x', unnest(ARRAY[n.name])) AS data FROM tb_ntop n $$);
DO $$ BEGIN
    IF (SELECT cascade_kinds->>'tb_badge' FROM tviews.registry WHERE entity = 'nvol') <> 'all_keys' THEN
        RAISE EXCEPTION '#182 FAIL: a volatile computed join classifies %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'nvol');
    END IF;
    IF (SELECT plan->'tables'->0->>'reason' FROM tviews.pg_tview_meta WHERE entity = 'ntop')
       <> 'read under a set-returning function in the top-level SELECT' THEN
        RAISE EXCEPTION '#182 FAIL: the top-level SRF control classifies %',
            (SELECT plan->'tables' FROM tviews.pg_tview_meta WHERE entity = 'ntop');
    END IF;
END $$;

-- ── a costly mapping names the index that avoids it ─────────────────────────
CREATE TABLE tb_area (pk_area bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      path text NOT NULL, name text);
INSERT INTO tb_area (pk_area, path, name) SELECT g, '1.' || g, 'a' || g FROM generate_series(1, 3000) g;
ANALYZE tb_area;
SET client_min_messages TO NOTICE;
SELECT pg_tviews_create('tv_area', $$
    SELECT n.pk_area, n.id, jsonb_build_object('names', array_agg(a.name ORDER BY a.pk_area)) AS data
    FROM tb_area n JOIN tb_area a ON a.pk_area = ANY (string_to_array(n.path, '.')::bigint[])
    GROUP BY n.pk_area, n.id $$);
SET client_min_messages TO WARNING;
-- The index it names makes the mapping query use it.
CREATE INDEX tb_area_path_ids ON tb_area USING gin ((string_to_array(path, '.')::bigint[]));
ANALYZE tb_area;
SET enable_seqscan = off;
DO $$
DECLARE plan text := '';
DECLARE line text;
BEGIN
    FOR line IN EXECUTE 'EXPLAIN WITH pg_tviews_delta AS (SELECT * FROM tb_area WHERE pk_area = 7) '
                        || tviews.pg_tviews_mapping_query('tv_area', 'tb_area'::regclass) LOOP
        plan := plan || line || E'\n';
    END LOOP;
    IF plan NOT LIKE '%tb_area_path_ids%' THEN
        RAISE EXCEPTION '#182 FAIL: the mapping query does not use the index it names:%', E'\n' || plan;
    END IF;
END $$;
RESET enable_seqscan;
UPDATE tb_area SET name = 'renamed' WHERE pk_area = 1;
SELECT assert_fresh('tv_area', 'pk_area', 'the rename of the area every path holds');

\echo 'issue #182 array joins: PASS'
