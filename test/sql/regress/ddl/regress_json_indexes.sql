-- Regression test (#226): declared JSON-path indexes.
--
-- Option `indexes`: an array of indexes, each an array of keys; a key is a path
-- ("status", "author.id") or {"path", "type", "desc"} with type text (default),
-- bigint, numeric or boolean. pg_tviews creates expression btrees that match the
-- SQL FraiseQL writes (data->>'k', data->'a'->>'b', (data->>'k')::bigint), keeps
-- them through replaces and rebuilds, and publishes them.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_json_indexes.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_item (
    pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user, status text, age int, price numeric, active boolean,
    created_at timestamptz);
INSERT INTO tb_user SELECT g, gen_random_uuid(), 'u' || g FROM generate_series(1, 100) g;
INSERT INTO tb_item SELECT g, gen_random_uuid(), 1 + g % 100, 's' || (g % 50), g % 90, g / 7.0,
                           g % 2 = 0, timestamptz '2026-01-01' + g * interval '1 minute'
FROM generate_series(1, 10000) g;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#226 FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $f$
DECLARE msg text; state text;
BEGIN
    EXECUTE stmt;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, state = RETURNED_SQLSTATE;
    RETURN state || ' ' || msg;
END $f$;
-- The plan of `q`, one line.
CREATE FUNCTION plan_of(q text) RETURNS text LANGUAGE plpgsql AS $f$
DECLARE line text; out text := '';
BEGIN
    FOR line IN EXECUTE 'EXPLAIN (COSTS OFF) ' || q LOOP out := out || line || ' | '; END LOOP;
    RETURN out;
END $f$;
CREATE FUNCTION indexes_of(tv regclass) RETURNS text LANGUAGE sql AS $$
    SELECT string_agg(c.relname, ',' ORDER BY c.relname)
    FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid
    WHERE i.indrelid = tv AND c.relname LIKE 'idx_%_data_%' $$;

\set def 'SELECT i.pk_item, i.id, i.fk_user, jsonb_build_object(''status'', i.status, ''age'', i.age, ''price'', i.price, ''active'', i.active, ''created_at'', i.created_at, ''author'', jsonb_build_object(''id'', u.id, ''name'', u.name)) AS data FROM tb_item i JOIN tb_user u ON u.pk_user = i.fk_user'

SELECT tviews.pg_tviews_create('tv_item', :'def', '{"indexes": [
    ["status", "created_at"],
    ["author.id"],
    [{"path": "age", "type": "bigint"}],
    [{"path": "price", "type": "numeric", "desc": true}],
    [{"path": "active", "type": "boolean"}]]}');
ANALYZE tv_item;
SET enable_seqscan = off;
SET enable_bitmapscan = off;

-- 1. Each FraiseQL shape is served by its index.
SELECT must(plan_of($$SELECT data FROM tv_item WHERE data->>'status' = 's7'$$) LIKE '%Index%idx_tv_item_data_status_created_at%',
            'top-level text: ' || plan_of($$SELECT data FROM tv_item WHERE data->>'status' = 's7'$$));
SELECT must(plan_of($$SELECT data FROM tv_item WHERE data->>'status' = 's7' ORDER BY data->>'created_at' LIMIT 5$$) LIKE '%Index%idx_tv_item_data_status_created_at%',
            'filter + text sort');
SELECT must(plan_of($$SELECT data FROM tv_item WHERE data->'author'->>'id' = 'x'$$) LIKE '%Index%idx_tv_item_data_author_id%',
            'nested: ' || plan_of($$SELECT data FROM tv_item WHERE data->'author'->>'id' = 'x'$$));
SELECT must(plan_of($$SELECT data FROM tv_item WHERE (data->>'age')::bigint > 80$$) LIKE '%Index%idx_tv_item_data_age_bigint%',
            'bigint: ' || plan_of($$SELECT data FROM tv_item WHERE (data->>'age')::bigint > 80$$));
SELECT must(plan_of($$SELECT data FROM tv_item ORDER BY (data->>'price')::numeric DESC LIMIT 3$$) LIKE '%Index Scan using idx_tv_item_data_price_numeric_desc%',
            'numeric desc sort: ' || plan_of($$SELECT data FROM tv_item ORDER BY (data->>'price')::numeric DESC LIMIT 3$$));
SELECT must(plan_of($$SELECT data FROM tv_item WHERE (data->>'active')::boolean = true$$) LIKE '%Index%idx_tv_item_data_active_boolean%',
            'boolean');
RESET enable_seqscan;
RESET enable_bitmapscan;

-- 2. Published as declared, and recorded as pg_tviews' own.
SELECT must((SELECT options->'indexes' FROM tviews.registry WHERE entity = 'item') =
    '[["status", "created_at"], ["author.id"], [{"path": "age", "type": "bigint"}],
      [{"path": "price", "type": "numeric", "desc": true}], [{"path": "active", "type": "boolean"}]]'::jsonb,
    'registry: ' || (SELECT options->'indexes' FROM tviews.registry WHERE entity = 'item')::text);
SELECT must((SELECT count(*) FROM tviews.registry r, unnest(r.managed_indexes) m
             WHERE r.entity = 'item' AND m::text LIKE '%_data_%') = 5, 'managed_indexes');
SELECT must(tviews.pg_tviews_create_or_replace('tv_item', query, options) = 'unchanged', 'round trip')
FROM tviews.registry WHERE entity = 'item';

-- 3. A changed declaration changes only the difference, in place.
SELECT (SELECT oid FROM pg_class WHERE relname = 'idx_tv_item_data_author_id') AS kept_oid \gset
SELECT must(tviews.pg_tviews_create_or_replace('tv_item', :'def', '{"indexes": [["author.id"], ["status"]]}') = 'altered',
            'changed declaration');
SELECT must(indexes_of('tv_item') = 'idx_tv_item_data_author_id,idx_tv_item_data_status', 'after change: ' || indexes_of('tv_item'));
SELECT must((SELECT oid FROM pg_class WHERE relname = 'idx_tv_item_data_author_id') = :kept_oid, 'kept index untouched');
-- A declared index dropped by hand comes back on the next replace.
DROP INDEX idx_tv_item_data_status;
SELECT must(tviews.pg_tviews_create_or_replace('tv_item', :'def', '{"indexes": [["author.id"], ["status"]]}') = 'altered',
            'missing index re-created');
SELECT must(indexes_of('tv_item') = 'idx_tv_item_data_author_id,idx_tv_item_data_status', 'recreated');
-- A rebuild (a column added) keeps them.
SELECT must(tviews.pg_tviews_create_or_replace('tv_item', replace(:'def', 'i.fk_user,', 'i.fk_user, i.age AS age,'),
            '{"indexes": [["author.id"], ["status"]]}') = 'rebuilt', 'rebuild');
SELECT must(indexes_of('tv_item') = 'idx_tv_item_data_author_id,idx_tv_item_data_status', 'after rebuild: ' || coalesce(indexes_of('tv_item'), 'none'));
-- No option: none.
SELECT tviews.pg_tviews_create_or_replace('tv_item', replace(:'def', 'i.fk_user,', 'i.fk_user, i.age AS age,'));
SELECT must(indexes_of('tv_item') IS NULL, 'dropped with the option');
SELECT tviews.pg_tviews_create_or_replace('tv_item', :'def', '{"indexes": [["status"]]}');

-- 4. The names are pg_tviews': another index under a declared index's name is
--    refused; exactly the declared index under it (what pg_restore runs) is
--    taken as pg_tviews'.
DROP INDEX idx_tv_item_data_status;
SELECT must(error_of('CREATE INDEX idx_tv_item_data_status ON tv_item ((data->>''name''))') LIKE '42939%',
            'reserved name: ' || coalesce(error_of('CREATE INDEX idx_tv_item_data_status ON tv_item ((data->>''name''))'), 'accepted'));
CREATE INDEX idx_tv_item_data_status ON public.tv_item USING btree (((data ->> 'status'::text)));
SELECT must('idx_tv_item_data_status'::regclass = ANY (managed_indexes), 'restored index recorded')
FROM tviews.registry WHERE entity = 'item';
SELECT must(tviews.pg_tviews_create_or_replace('tv_item', :'def', '{"indexes": [["status"]]}') = 'unchanged',
            'restored index is the declared one');

-- 5. Refused declarations.
CREATE TABLE def AS SELECT :'def'::text AS q;
CREATE FUNCTION refused(options text) RETURNS text LANGUAGE sql AS $$
    SELECT error_of(format('SELECT tviews.pg_tviews_create_or_replace(%L, %L, %L)',
                           'tv_item', (SELECT q FROM def), options)) $$;
SELECT must(refused('{"indexes": [[{"path": "created_at", "type": "timestamptz"}]]}')
            LIKE '22023%timestamptz is not immutable%', 'timestamptz refused');
SELECT must(refused('{"indexes": ["status"]}') LIKE '22023%indexes%', 'shape refused');
SELECT must(refused('{"indexes": [["a..b"]]}') LIKE '22023%empty segment%', 'empty path segment refused');
SELECT must(refused('{"indexes": [["status"], ["status"]]}') LIKE '22023%twice%', 'duplicate refused');
SELECT must(refused('{"indexes": [[{"path": "age", "kind": "x"}]]}') LIKE '22023%kind%', 'unknown key field refused');
SELECT must((SELECT options->'indexes' FROM tviews.registry WHERE entity = 'item') = '[["status"]]',
            'a refused declaration changes nothing');

-- 6. A quote in a path is a literal, not SQL.
SELECT tviews.pg_tviews_create_or_replace('tv_item', :'def', '{"indexes": [["it''s"]]}');
SELECT must((SELECT count(*) FROM pg_indexes WHERE tablename = 'tv_item' AND indexdef LIKE '%it''''s%') = 1, 'quoted path');

SELECT 'issue #226 json indexes: PASS' AS result;
-- expect-output: issue #226 json indexes: PASS
