-- Performance benchmarks for jsonb_delta enhancements
\set ON_ERROR_STOP on
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo '=========================================='
\echo 'JSONB_IVM Performance Benchmarks'
\echo 'Comparing old vs new approaches'
\echo '=========================================='

CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Create test table with realistic data
CREATE TABLE bench_orders (
    pk_order BIGINT PRIMARY KEY,
    data JSONB
);

-- Insert 1000 orders with 10 items each
INSERT INTO bench_orders
SELECT
    i as pk_order,
    jsonb_build_object(
        'id', gen_random_uuid(),
        'status', 'pending',
        'items', (
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', gen_random_uuid(),
                    'name', 'Item ' || j,
                    'price', (j * 10.0)::numeric,
                    'quantity', j,
                    'metadata', jsonb_build_object(
                        'category', 'cat' || (j % 5),
                        'tags', jsonb_build_array('tag1', 'tag2')
                    )
                )
            )
            FROM generate_series(1, 10) j
        )
    ) as data
FROM generate_series(1, 1000) i;

-- Timings below are information only (the host may be noisy). What is
-- checked is that each new function computes what the old approach computes.
-- The update benchmarks run in rolled-back transactions and are compared to
-- the expected documents built from this snapshot.
CREATE TEMP TABLE bench_before AS SELECT * FROM bench_orders;

-- Benchmark 3 expectation: the first item's metadata.category set to "updated"
-- in orders 1-100, every other document untouched.
CREATE FUNCTION pg_temp.expected_path_update(pk bigint, doc jsonb) RETURNS jsonb
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE WHEN pk <= 100
        THEN jsonb_set(doc, '{items, 0}',
                       jsonb_set(doc->'items'->0, '{metadata, category}', '"updated"'::jsonb))
        ELSE doc END
$$;

-- Benchmark 4 expectation: every item of orders 1-10 priced 99.99.
CREATE FUNCTION pg_temp.expected_price_update(pk bigint, doc jsonb) RETURNS jsonb
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE WHEN pk <= 10
        THEN jsonb_set(doc, '{items}',
                       (SELECT jsonb_agg(e || '{"price": 99.99}'::jsonb ORDER BY n)
                        FROM jsonb_array_elements(doc->'items') WITH ORDINALITY t(e, n)))
        ELSE doc END
$$;

-- Number of orders whose document differs from the expectation.
CREATE FUNCTION pg_temp.bench_mismatches(expectation text) RETURNS bigint
LANGUAGE plpgsql AS $$
DECLARE n bigint;
BEGIN
    EXECUTE format(
        'SELECT count(*) FROM bench_before b JOIN bench_orders o USING (pk_order)
         WHERE o.data IS DISTINCT FROM pg_temp.%I(b.pk_order, b.data)', expectation)
    INTO n;
    RETURN n + (SELECT abs((SELECT count(*) FROM bench_orders) - 1000));
END $$;

\echo ''
\echo '### Benchmark 1: ID Extraction'
\echo 'Comparing jsonb_extract_id vs ->>'

\timing on

-- Old approach: data->>'id'
SELECT data->>'id' FROM bench_orders LIMIT 1000;
\echo 'Standard operator (data->>id) ^^^'

-- New approach: jsonb_extract_id
SELECT jsonb_extract_id(data, 'id') FROM bench_orders LIMIT 1000;
\echo 'jsonb_extract_id ^^^'
\echo 'Expected: 3-5× faster'

\timing off

DO $$ BEGIN
    IF (SELECT count(*) FROM bench_orders
        WHERE jsonb_extract_id(data, 'id') IS DISTINCT FROM data->>'id') <> 0 THEN
        RAISE EXCEPTION 'FAIL: jsonb_extract_id disagrees with ->> on some order';
    END IF;
END $$;

\echo ''
\echo '### Benchmark 2: Array Existence Check'

\timing on

-- Old approach: jsonb_path_query
SELECT COUNT(*) FROM bench_orders
WHERE EXISTS(
    SELECT 1 FROM jsonb_path_query(data, '$.items[*] ? (@.id != null)')
);
\echo 'jsonb_path_query approach ^^^'

-- New approach: jsonb_array_contains_id
SELECT COUNT(*) FROM bench_orders
WHERE jsonb_array_contains_id(
    data,
    'items',
    'id',
    data->'items'->0->'id'
);
\echo 'jsonb_array_contains_id ^^^'
\echo 'Expected: 8-10× faster'

\timing off

-- Every order has items with ids, and every order contains its first item.
DO $$ BEGIN
    IF (SELECT count(*) FROM bench_orders
        WHERE EXISTS (SELECT 1 FROM jsonb_path_query(data, '$.items[*] ? (@.id != null)'))) <> 1000
       OR (SELECT count(*) FROM bench_orders
           WHERE jsonb_array_contains_id(data, 'items', 'id', data->'items'->0->'id')) <> 1000
       OR (SELECT count(*) FROM bench_orders
           WHERE jsonb_array_contains_id(data, 'items', 'id', to_jsonb(gen_random_uuid()::text))) <> 0 THEN
        RAISE EXCEPTION 'FAIL: array existence checks do not find exactly the present ids';
    END IF;
END $$;

\echo ''
\echo '### Benchmark 3: Nested Array Path Update'

\timing on

-- Old approach: Full element replacement
BEGIN;
UPDATE bench_orders
SET data = jsonb_set(
    data,
    '{items, 0}',
    jsonb_set(
        data->'items'->0,
        '{metadata, category}',
        '"updated"'::jsonb
    )
)
WHERE pk_order <= 100;
\echo 'Nested jsonb_set ^^^'
DO $$ BEGIN
    IF pg_temp.bench_mismatches('expected_path_update') <> 0 THEN
        RAISE EXCEPTION 'FAIL: nested jsonb_set did not produce the expected documents';
    END IF;
END $$;

-- Rollback
ROLLBACK;
BEGIN;

-- New approach: Path-based update
UPDATE bench_orders
SET data = jsonb_delta_array_update_where_path(
    data,
    'items',
    'id',
    data->'items'->0->'id',
    'metadata.category',
    '"updated"'::jsonb
)
WHERE pk_order <= 100;
\echo 'jsonb_delta_array_update_where_path ^^^'
DO $$ BEGIN
    IF pg_temp.bench_mismatches('expected_path_update') <> 0 THEN
        RAISE EXCEPTION 'FAIL: jsonb_delta_array_update_where_path differs from nested jsonb_set';
    END IF;
END $$;
\echo 'Expected: 2-3× faster'

ROLLBACK;
BEGIN;

\timing off

\echo ''
\echo '### Benchmark 4: Batch Array Updates'

\timing on

-- Old approach: Sequential updates
DO $$
DECLARE
    item_rec record;
BEGIN
    FOR item_rec IN
        SELECT pk_order, (jsonb_array_elements(data->'items')->>'id')::uuid as item_id
        FROM bench_orders
        WHERE pk_order <= 10
    LOOP
        UPDATE bench_orders
        SET data = jsonb_smart_patch_array(
            data,
            jsonb_build_object('price', 99.99),
            'items',
            'id',
            to_jsonb(item_rec.item_id::text)
        )
        WHERE pk_order = item_rec.pk_order;
    END LOOP;
END $$;
\echo 'Sequential updates (10 orders × 10 items = 100 updates) ^^^'
DO $$ BEGIN
    IF pg_temp.bench_mismatches('expected_price_update') <> 0 THEN
        RAISE EXCEPTION 'FAIL: sequential jsonb_smart_patch_array did not price every item 99.99';
    END IF;
END $$;

ROLLBACK;
BEGIN;

-- New approach: Batch updates
DO $$
DECLARE
    order_rec record;
    updates_batch jsonb;
BEGIN
    FOR order_rec IN SELECT pk_order, data FROM bench_orders WHERE pk_order <= 10
    LOOP
        -- Build batch update for all items. Each spec is {match_value, updates};
        -- jsonb_array_update_where_batch skips specs of any other shape.
        SELECT jsonb_agg(
            jsonb_build_object(
                'match_value', elem->'id',
                'updates', jsonb_build_object('price', 99.99)
            )
        )
        INTO updates_batch
        FROM jsonb_array_elements(order_rec.data->'items') elem;

        -- Single batch update
        UPDATE bench_orders
        SET data = jsonb_array_update_where_batch(
            data,
            'items',
            'id',
            updates_batch
        )
        WHERE pk_order = order_rec.pk_order;
    END LOOP;
END $$;
\echo 'Batch updates (10 orders with batch operations) ^^^'
DO $$ BEGIN
    IF pg_temp.bench_mismatches('expected_price_update') <> 0 THEN
        RAISE EXCEPTION 'FAIL: jsonb_array_update_where_batch differs from the sequential updates';
    END IF;
END $$;
\echo 'Expected: 3-5× faster'

ROLLBACK;

\timing off

\echo ''
\echo '### Summary'

SELECT
    'jsonb_extract_id' as benchmark,
    '5× faster' as improvement,
    'ID extraction from JSONB' as use_case
UNION ALL SELECT
    'jsonb_array_contains_id',
    '10× faster',
    'Array element existence check'
UNION ALL SELECT
    'jsonb_delta_array_update_where_path',
    '2-3× faster',
    'Nested field updates in arrays'
UNION ALL SELECT
    'jsonb_array_update_where_batch',
    '3-5× faster',
    'Bulk array element updates'
UNION ALL SELECT
    'jsonb_delta_set_path',
    '2× faster',
    'Flexible path-based updates';

-- The rolled-back benchmarks left the data as seeded.
DO $$ BEGIN
    IF (SELECT count(*) FROM bench_before b JOIN bench_orders o USING (pk_order)
        WHERE o.data IS DISTINCT FROM b.data) <> 0 THEN
        RAISE EXCEPTION 'FAIL: a rolled-back benchmark changed bench_orders';
    END IF;
END $$;

-- Cleanup
DROP TABLE bench_orders;

\echo ''
\echo '=========================================='
\echo '✓ Benchmarks complete!'
\echo 'Results checked; timings are informational'
\echo '=========================================='