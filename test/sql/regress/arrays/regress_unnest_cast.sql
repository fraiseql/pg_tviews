-- Regression test for issue #196: a join through `unnest(<array>)::T` (a cast
-- around unnest, in a CTE or a subquery), or through a cast of a LATERAL unnest
-- element, is mapped like #182's spellings: each element of the unnested array
-- cast one by one is an element of `(<array>)::T[]`.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/arrays/regress_unnest_cast.sql
--
-- expect-once: CREATE INDEX ON public.tb_area USING gin
-- expect-output: issue #196 unnest cast: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#196 FAIL: %', what; END IF; END $$;

SET pg_tviews.uncascaded_policy = 'error';

-- One copy of the tree per spelling.
DO $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY ARRAY['ncte', 'nsub', 'nlat', 'ntxt'] LOOP
        EXECUTE format('CREATE TABLE tb_%1$s (pk_%1$s bigint PRIMARY KEY,
                            id uuid NOT NULL DEFAULT gen_random_uuid(), name text, path text)', e);
        EXECUTE format($q$INSERT INTO tb_%1$s (pk_%1$s, name, path)
                        VALUES (1, 'root', '1'), (2, 'mid', '1.2'), (3, 'leaf', '1.2.3'), (4, 'other', '1.4')$q$, e);
    END LOOP;
END $$;
CREATE FUNCTION write_all(stmt text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY ARRAY['ncte', 'nsub', 'nlat', 'ntxt'] LOOP
        EXECUTE replace(replace(stmt, 'tb_x', 'tb_' || e), 'pk_x', 'pk_' || e);
    END LOOP;
END $$;
CREATE FUNCTION check_all(label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY ARRAY['ncte', 'nsub', 'nlat', 'ntxt'] LOOP
        PERFORM must(fresh_diff(('tv_' || e)::regclass, 'pk_' || e) IS NULL,
                     fresh_diff(('tv_' || e)::regclass, 'pk_' || e) || ' after ' || label);
    END LOOP;
END $$;

-- The issue: the cast outside unnest, in a CTE.
SELECT tviews.pg_tviews_create('tv_ncte', $$
WITH node_ids AS (
  SELECT n.pk_ncte, n.id, unnest(string_to_array(n.path, '.'))::bigint AS node_id FROM tb_ncte n)
SELECT x.pk_ncte, x.id, array_agg(a.name ORDER BY a.pk_ncte) AS names
FROM node_ids x JOIN tb_ncte a ON a.pk_ncte = x.node_id
GROUP BY x.pk_ncte, x.id $$);
-- The same in a subquery.
SELECT tviews.pg_tviews_create('tv_nsub', $$
SELECT x.pk_nsub, x.id, array_agg(a.name ORDER BY a.pk_nsub) AS names
FROM (SELECT n.pk_nsub, n.id, unnest(string_to_array(n.path, '.'))::bigint AS node_id FROM tb_nsub n) x
JOIN tb_nsub a ON a.pk_nsub = x.node_id
GROUP BY x.pk_nsub, x.id $$);
-- A LATERAL unnest of text, its element cast in the join condition.
SELECT tviews.pg_tviews_create('tv_nlat', $$
SELECT n.pk_nlat, n.id, array_agg(a.name ORDER BY a.pk_nlat) AS names
FROM tb_nlat n CROSS JOIN LATERAL unnest(string_to_array(n.path, '.')) AS u(x)
JOIN tb_nlat a ON a.pk_nlat = u.x::bigint
GROUP BY n.pk_nlat, n.id $$);
-- An int array unnested and cast to text, joined on a text column.
SELECT tviews.pg_tviews_create('tv_ntxt', $$
SELECT x.pk_ntxt, x.id, array_agg(a.name ORDER BY a.pk_ntxt) AS names
FROM (SELECT n.pk_ntxt, n.id, unnest(string_to_array(n.path, '.')::bigint[])::text AS node_key FROM tb_ntxt n) x
JOIN tb_ntxt a ON a.pk_ntxt::text = x.node_key
GROUP BY x.pk_ntxt, x.id $$);

SELECT must(cascade_kinds = jsonb_build_object('tb_' || entity, 'mapped') AND cardinality(uncascaded_tables) = 0,
            'tv_' || entity || ' classifies ' || cascade_kinds::text)
FROM tviews.registry WHERE entity IN ('ncte', 'nsub', 'nlat', 'ntxt');
SELECT must(tviews.pg_tviews_mapping_query('tv_ncte', 'tb_ncte'::regclass) LIKE '%ANY%',
            'the mapping of tv_ncte is ' || tviews.pg_tviews_mapping_query('tv_ncte', 'tb_ncte'::regclass));

SELECT write_all($$UPDATE tb_x SET name = 'MID' WHERE pk_x = 2$$);
SELECT check_all('the rename of a mid node');
SELECT write_all($$UPDATE tb_x SET path = '1.4.3' WHERE pk_x = 3$$);
SELECT check_all('the move of node 3 under node 4');
SELECT write_all($$INSERT INTO tb_x (pk_x, name, path) VALUES (5, 'new', '1.4.5')$$);
SELECT check_all('an insert');
SELECT write_all('DELETE FROM tb_x WHERE pk_x = 4');
SELECT check_all('the delete of node 4');

-- A costly mapping names the GIN index on the cast array, and the mapping uses it.
CREATE TABLE tb_area (pk_area bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      path text NOT NULL, name text);
INSERT INTO tb_area (pk_area, path, name) SELECT g, '1.' || g, 'a' || g FROM generate_series(1, 3000) g;
ANALYZE tb_area;
SET client_min_messages TO NOTICE;
SELECT tviews.pg_tviews_create('tv_area', $$
WITH area_ids AS (
  SELECT n.pk_area, n.id, unnest(string_to_array(n.path, '.'))::bigint AS area_id FROM tb_area n)
SELECT x.pk_area, x.id, array_agg(a.name ORDER BY a.pk_area) AS names
FROM area_ids x JOIN tb_area a ON a.pk_area = x.area_id
GROUP BY x.pk_area, x.id $$);
SET client_min_messages TO WARNING;
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
    PERFORM must(plan LIKE '%tb_area_path_ids%', 'the mapping query does not use the index it names:' || E'\n' || plan);
END $$;
RESET enable_seqscan;
UPDATE tb_area SET name = 'renamed' WHERE pk_area = 1;
SELECT assert_fresh('tv_area', 'pk_area', 'the rename of the area every path holds');

\echo issue #196 unnest cast: PASS
