-- A window function, LIMIT/OFFSET, a set-returning function or GROUPING SETS in the
-- backing view's top-level SELECT: a changed row does not map to its own key only
-- (a window or LIMIT changes other rows), so every table read there is `all_keys`
-- and the TVIEW's stored uncascaded_policy applies. INTERSECT and EXCEPT keep
-- mapping each branch like UNION, with no WARNING.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_top_level_opaque_shapes.sql
--
-- expect-output: writes to public.tb_warned will not refresh public.tv_warned
-- expect-output: read under a window function in the top-level SELECT
-- expect-output: top-level opaque shapes: PASS
-- reject-output: will not refresh public.tv_both
-- reject-output: will not refresh public.tv_left_only

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- One table per TVIEW (tb_<entity> with pk_<entity>), same rows in each.
DO $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY ARRAY['win', 'rank', 'top', 'srf', 'gs', 'warned', 'refused', 'ordered'] LOOP
        EXECUTE format('CREATE TABLE tb_%1$s (pk_%1$s bigint PRIMARY KEY,
                            id uuid NOT NULL DEFAULT gen_random_uuid(), g int NOT NULL, s int NOT NULL)', e);
        EXECUTE format('INSERT INTO tb_%1$s (pk_%1$s, g, s)
                        VALUES (1, 1, 10), (2, 1, 20), (3, 2, 30), (4, 2, 40)', e);
    END LOOP;
END $$;

-- Write the same change to the tables of the given TVIEWs.
CREATE FUNCTION write_all(entities text[], stmt text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE e text;
BEGIN
    FOREACH e IN ARRAY entities LOOP
        EXECUTE replace(replace(stmt, 'tb_x', 'tb_' || e), 'pk_x', 'pk_' || e);
    END LOOP;
END $$;

CREATE FUNCTION check_fresh(tv text, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE d bigint;
BEGIN
    EXECUTE format(
        'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s EXCEPT SELECT pk_%1$s, data FROM v_%1$s)
                     UNION ALL (SELECT pk_%1$s, data FROM v_%1$s EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d',
        tv) INTO d;
    IF d <> 0 THEN
        RAISE EXCEPTION 'item 6 FAIL: tv_% stale after %', tv, label;
    END IF;
END $$;

-- ── behaviour under full_refresh ────────────────────────────────────────────
SET pg_tviews.uncascaded_policy = 'full_refresh';
SET client_min_messages TO ERROR;
SELECT pg_tviews_create('tv_win', $$
    SELECT pk_win, id, jsonb_build_object('s', s, 'n', count(*) OVER ()) AS data FROM tb_win $$);
SELECT pg_tviews_create('tv_rank', $$
    SELECT pk_rank, id, jsonb_build_object('rank', row_number() OVER (PARTITION BY g ORDER BY s)) AS data
    FROM tb_rank $$);
SELECT pg_tviews_create('tv_top', $$
    SELECT pk_top, id, jsonb_build_object('s', s) AS data FROM tb_top ORDER BY s DESC LIMIT 2 $$);
SELECT pg_tviews_create('tv_srf', $$
    SELECT pk_srf, id, jsonb_build_object('x', unnest(ARRAY[s])) AS data FROM tb_srf $$);
SELECT pg_tviews_create('tv_gs', $$
    SELECT pk_gs, id, jsonb_build_object('n', count(*), 's', sum(s)) AS data
    FROM tb_gs GROUP BY GROUPING SETS ((pk_gs, id), ()) HAVING GROUPING(pk_gs, id) = 0 $$);
SET client_min_messages TO WARNING;

-- One statement per table: the write to a table is what refreshes its TVIEW.
SELECT write_all(ARRAY['win', 'rank', 'top', 'srf', 'gs'],
                 'INSERT INTO tb_x (pk_x, g, s) VALUES (5, 1, 15), (6, 3, 50)');
SELECT check_fresh(e, 'INSERT') FROM unnest(ARRAY['win', 'rank', 'top', 'srf', 'gs']) e;
SELECT write_all(ARRAY['win', 'rank', 'top', 'srf', 'gs'], 'UPDATE tb_x SET s = 5 WHERE pk_x = 6');
SELECT check_fresh(e, 'UPDATE') FROM unnest(ARRAY['win', 'rank', 'top', 'srf', 'gs']) e;
SELECT write_all(ARRAY['win', 'rank', 'top', 'srf', 'gs'], 'DELETE FROM tb_x WHERE pk_x = 2');
SELECT check_fresh(e, 'DELETE') FROM unnest(ARRAY['win', 'rank', 'top', 'srf', 'gs']) e;

-- ── classification ──────────────────────────────────────────────────────────
DO $$
DECLARE r record;
BEGIN
    FOR r IN SELECT entity, cascade_kinds, uncascaded_tables FROM tviews.registry
             WHERE entity IN ('win', 'rank', 'top', 'srf', 'gs') LOOP
        IF r.cascade_kinds->>('tb_' || r.entity) IS DISTINCT FROM 'all_keys'
           OR NOT r.uncascaded_tables::text LIKE '%tb_' || r.entity || '%' THEN
            RAISE EXCEPTION 'item 6 FAIL: tv_% classifies its table % (uncascaded: %)',
                r.entity, r.cascade_kinds, r.uncascaded_tables;
        END IF;
    END LOOP;
END $$;

-- ── warn and error ──────────────────────────────────────────────────────────
SET pg_tviews.uncascaded_policy = 'warn';
SELECT pg_tviews_create('tv_warned', $$
    SELECT pk_warned, id, jsonb_build_object('n', count(*) OVER ()) AS data FROM tb_warned $$);
SET pg_tviews.uncascaded_policy = 'error';
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_refused', $q$
        SELECT pk_refused, id, jsonb_build_object('s', s) AS data FROM tb_refused ORDER BY s LIMIT 1 $q$);
    RAISE EXCEPTION 'item 6 FAIL: a top-level LIMIT was accepted under the error policy';
EXCEPTION WHEN OTHERS THEN
    IF SQLERRM LIKE 'item 6 FAIL%' THEN RAISE; END IF;
END $$;
RESET pg_tviews.uncascaded_policy;

-- Not opaque: ORDER BY alone stays local.
SELECT pg_tviews_create('tv_ordered', $$
    SELECT pk_ordered, id, jsonb_build_object('s', s) AS data FROM tb_ordered ORDER BY s $$);
DO $$ BEGIN
    IF (SELECT cascade_kinds->>'tb_ordered' FROM tviews.registry WHERE entity = 'ordered') <> 'local' THEN
        RAISE EXCEPTION 'item 6 FAIL: ORDER BY alone made tb_ordered %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'ordered');
    END IF;
END $$;

-- ── INTERSECT and EXCEPT map each branch, with no WARNING ───────────────────
CREATE TABLE tb_both (pk_both bigint PRIMARY KEY, id uuid NOT NULL, v text);
CREATE TABLE tb_left_only (pk_left_only bigint PRIMARY KEY, id uuid NOT NULL, v text);
CREATE TABLE tb_peer (pk_peer bigint PRIMARY KEY, id uuid NOT NULL, v text);
INSERT INTO tb_both SELECT g, md5(g::text)::uuid, 'v' || g FROM generate_series(1, 4) g;
INSERT INTO tb_left_only SELECT g, md5(g::text)::uuid, 'v' || g FROM generate_series(1, 4) g;
INSERT INTO tb_peer SELECT g, md5(g::text)::uuid, 'v' || g FROM generate_series(3, 6) g;
SELECT pg_tviews_create('tv_both', $$
    SELECT pk_both, id, jsonb_build_object('v', v) AS data FROM tb_both
    INTERSECT
    SELECT pk_peer, id, jsonb_build_object('v', v) FROM tb_peer $$);
SELECT pg_tviews_create('tv_left_only', $$
    SELECT pk_left_only, id, jsonb_build_object('v', v) AS data FROM tb_left_only
    EXCEPT
    SELECT pk_peer, id, jsonb_build_object('v', v) FROM tb_peer $$);
SELECT write_all(ARRAY['both', 'left_only'], $q$INSERT INTO tb_x VALUES (5, md5('5')::uuid, 'v5')$q$);
SELECT check_fresh(e, 'INSERT left') FROM unnest(ARRAY['both', 'left_only']) e;
INSERT INTO tb_peer VALUES (1, md5('1')::uuid, 'v1');
SELECT check_fresh(e, 'INSERT right') FROM unnest(ARRAY['both', 'left_only']) e;
DELETE FROM tb_peer WHERE pk_peer = 3;
SELECT check_fresh(e, 'DELETE right') FROM unnest(ARRAY['both', 'left_only']) e;
SELECT write_all(ARRAY['both', 'left_only'], $q$UPDATE tb_x SET v = 'changed' WHERE pk_x = 4$q$);
SELECT check_fresh(e, 'UPDATE left') FROM unnest(ARRAY['both', 'left_only']) e;
SELECT write_all(ARRAY['both', 'left_only'], 'DELETE FROM tb_x WHERE pk_x = 1');
SELECT check_fresh(e, 'DELETE left') FROM unnest(ARRAY['both', 'left_only']) e;

\echo 'top-level opaque shapes: PASS'
