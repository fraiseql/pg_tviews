-- The jsonb_delta path and array helpers compute what the plain jsonb functions
-- compute, and the plain jsonb updates the recompute path builds documents with
-- keep sibling fields.
--
-- pg_tviews installs next to jsonb_delta (it is required): these are the
-- results its documents and users rely on from the helpers jsonb_delta_set_path,
-- jsonb_extract_id, jsonb_array_contains_id, jsonb_delta_array_update_where_path,
-- jsonb_smart_patch_array and jsonb_array_update_where_batch. Each update is
-- compared to the document a nested jsonb_set builds, and rolled back.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/direct_patch/regress_jsonb_delta_helper_results.sql
-- expect-output: jsonb_delta_helper_results: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'jsonb_delta helper results FAIL: %', what; END IF; END $$;

-- 1. jsonb_delta_set_path is installed with pg_tviews.
SELECT must(EXISTS (SELECT 1 FROM pg_proc WHERE proname = 'jsonb_delta_set_path'),
            'jsonb_delta_set_path is not available');

-- 2. jsonb_delta_set_path: a dotted path is replaced, its siblings kept.
SELECT must(jsonb_delta_set_path(
                '{"user":{"profile":{"name":"Alice","email":"a@old","settings":{"theme":"light"}}}}',
                'user.profile.email', '"a@new"')
            = '{"user":{"profile":{"name":"Alice","email":"a@new","settings":{"theme":"light"}}}}',
            'set_path: nested key');
-- An array index in the path; the element's other fields kept.
SELECT must(jsonb_delta_set_path(
                '{"items":[{"id":1,"metadata":{"tags":["t1","t2"],"status":"active"}}]}',
                'items[0].metadata.status', '"inactive"')
            = '{"items":[{"id":1,"metadata":{"tags":["t1","t2"],"status":"inactive"}}]}',
            'set_path: array index');
-- Chained calls.
SELECT must(jsonb_delta_set_path(jsonb_delta_set_path(jsonb_delta_set_path(
                '{"config":{"server":"prod","port":8080,"ssl":true}}',
                'config.server', '"staging"'), 'config.port', '9090'), 'config.ssl', 'false')
            = '{"config":{"server":"staging","port":9090,"ssl":false}}',
            'set_path: chained');
-- Missing intermediate objects are created.
SELECT must(jsonb_delta_set_path('{}', 'new.nested.deep.value', '"created"')
            = '{"new":{"nested":{"deep":{"value":"created"}}}}',
            'set_path: intermediate objects');

-- 3. Plain jsonb_set, the recompute path's building block: siblings kept.
SELECT must(jsonb_set('{"user":{"profile":{"name":"Bob","email":"b@old"}}}',
                      '{user,profile,email}', '"b@new"')
            = '{"user":{"profile":{"name":"Bob","email":"b@new"}}}',
            'jsonb_set: nested key');
SELECT must(jsonb_set(jsonb_set(
                '{"project":{"metadata":{"tags":["web","api"],"status":"draft"},"settings":{"visibility":"private"}}}',
                '{project,metadata,status}', '"published"'), '{project,settings,visibility}', '"public"')
            = '{"project":{"metadata":{"tags":["web","api"],"status":"published"},"settings":{"visibility":"public"}}}',
            'jsonb_set: two nested keys');
SELECT must(jsonb_set('{"name":"test"}', '{name}', '"updated"') = '{"name":"updated"}',
            'jsonb_set: top-level key');

-- 4. The array helpers, over 30 orders of 4 items each.
CREATE TABLE orders (pk_order bigint PRIMARY KEY, data jsonb);
INSERT INTO orders
SELECT i, jsonb_build_object(
    'id', gen_random_uuid(), 'status', 'pending',
    'items', (SELECT jsonb_agg(jsonb_build_object(
                  'id', gen_random_uuid(), 'name', 'Item ' || j, 'price', j * 10.0,
                  'metadata', jsonb_build_object('category', 'cat' || j % 5,
                                                 'tags', jsonb_build_array('tag1', 'tag2')))
                  ORDER BY j)
              FROM generate_series(1, 4) j))
FROM generate_series(1, 30) i;
CREATE TABLE before AS SELECT * FROM orders;

-- Orders 1-10: the first item's metadata.category set to "updated".
CREATE FUNCTION expected_path_update(pk bigint, doc jsonb) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE WHEN pk <= 10
        THEN jsonb_set(doc, '{items,0}', jsonb_set(doc->'items'->0, '{metadata,category}', '"updated"'))
        ELSE doc END $$;
-- Orders 1-5: every item priced 99.99.
CREATE FUNCTION expected_price_update(pk bigint, doc jsonb) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE WHEN pk <= 5
        THEN jsonb_set(doc, '{items}',
                       (SELECT jsonb_agg(e || '{"price": 99.99}' ORDER BY n)
                        FROM jsonb_array_elements(doc->'items') WITH ORDINALITY t(e, n)))
        ELSE doc END $$;
-- Orders whose document differs from `expectation` applied to the seeded one.
CREATE FUNCTION mismatches(expectation text) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE n bigint;
BEGIN
    EXECUTE format('SELECT count(*) FROM before b JOIN orders o USING (pk_order)
                    WHERE o.data IS DISTINCT FROM %I(b.pk_order, b.data)', expectation) INTO n;
    RETURN n + abs((SELECT count(*) FROM orders) - 30);
END $$;

-- jsonb_extract_id agrees with ->>.
SELECT must((SELECT count(*) FROM orders WHERE jsonb_extract_id(data, 'id') IS DISTINCT FROM data->>'id') = 0,
            'jsonb_extract_id disagrees with ->>');

-- jsonb_array_contains_id finds a present id and no absent one.
SELECT must((SELECT count(*) FROM orders
             WHERE EXISTS (SELECT 1 FROM jsonb_path_query(data, '$.items[*] ? (@.id != null)'))) = 30,
            'jsonpath existence');
SELECT must((SELECT count(*) FROM orders
             WHERE jsonb_array_contains_id(data, 'items', 'id', data->'items'->0->'id')) = 30,
            'jsonb_array_contains_id misses the first item');
SELECT must((SELECT count(*) FROM orders
             WHERE jsonb_array_contains_id(data, 'items', 'id', to_jsonb(gen_random_uuid()::text))) = 0,
            'jsonb_array_contains_id finds an absent id');

-- jsonb_delta_array_update_where_path equals the nested jsonb_set.
BEGIN;
UPDATE orders SET data = jsonb_set(data, '{items,0}',
                                   jsonb_set(data->'items'->0, '{metadata,category}', '"updated"'))
WHERE pk_order <= 10;
SELECT must(mismatches('expected_path_update') = 0, 'nested jsonb_set');
ROLLBACK;
BEGIN;
UPDATE orders SET data = jsonb_delta_array_update_where_path(
    data, 'items', 'id', data->'items'->0->'id', 'metadata.category', '"updated"'::jsonb)
WHERE pk_order <= 10;
SELECT must(mismatches('expected_path_update') = 0, 'jsonb_delta_array_update_where_path');
ROLLBACK;

-- jsonb_smart_patch_array, one item at a time, prices every item.
BEGIN;
DO $$
DECLARE r record;
BEGIN
    FOR r IN SELECT pk_order, (jsonb_array_elements(data->'items')->>'id')::uuid AS item_id
             FROM orders WHERE pk_order <= 5
    LOOP
        UPDATE orders SET data = jsonb_smart_patch_array(
            data, jsonb_build_object('price', 99.99), 'items', 'id', to_jsonb(r.item_id::text))
        WHERE pk_order = r.pk_order;
    END LOOP;
END $$;
SELECT must(mismatches('expected_price_update') = 0, 'sequential jsonb_smart_patch_array');
ROLLBACK;

-- jsonb_array_update_where_batch, one call per order, does the same.
BEGIN;
DO $$
DECLARE r record; batch jsonb;
BEGIN
    FOR r IN SELECT pk_order, data FROM orders WHERE pk_order <= 5 LOOP
        SELECT jsonb_agg(jsonb_build_object('match_value', e->'id',
                                            'updates', jsonb_build_object('price', 99.99)))
        INTO batch FROM jsonb_array_elements(r.data->'items') e;
        UPDATE orders SET data = jsonb_array_update_where_batch(data, 'items', 'id', batch)
        WHERE pk_order = r.pk_order;
    END LOOP;
END $$;
SELECT must(mismatches('expected_price_update') = 0, 'jsonb_array_update_where_batch');
ROLLBACK;

-- The rolled-back updates left the documents as seeded.
SELECT must((SELECT count(*) FROM before b JOIN orders o USING (pk_order)
             WHERE o.data IS DISTINCT FROM b.data) = 0, 'a rolled-back update changed orders');

\echo 'jsonb_delta_helper_results: PASS'
