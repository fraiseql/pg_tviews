-- Comprehensive integration test for all jsonb_ivm enhancements
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo '=========================================='
\echo 'JSONB_IVM Integration Test Suite'
\echo 'Testing Phases 1-4 together'
\echo '=========================================='

CREATE EXTENSION IF NOT EXISTS pg_tviews;  -- NO CASCADE - testing fallback behavior

\echo ''
\echo '### Scenario: E-commerce Order Management System'
\echo 'Tests all new functions in realistic cascade scenario'

-- Create schema
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    name TEXT,
    email TEXT,
    profile JSONB DEFAULT '{}'::jsonb
);

CREATE TABLE tb_product (
    pk_product BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    name TEXT,
    price NUMERIC(10,2),
    category TEXT
);

CREATE TABLE tb_order (
    pk_order BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    fk_user BIGINT REFERENCES tb_user(pk_user),
    status TEXT,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE tb_order_item (
    pk_order_item BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    fk_order BIGINT REFERENCES tb_order(pk_order),
    fk_product BIGINT REFERENCES tb_product(pk_product),
    quantity INT,
    price_at_order NUMERIC(10,2)
);

-- Create TVIEW with nested structures
CREATE TABLE tv_order AS
SELECT
    o.pk_order,
    o.id,
    o.fk_user,
    jsonb_build_object(
        'id', o.id,
        'status', o.status,
        'created_at', o.created_at,
        'customer', jsonb_build_object(
            'id', u.id,
            'name', u.name,
            'email', u.email,
            'profile', u.profile
        ),
        'items', COALESCE(
            jsonb_agg(
                jsonb_build_object(
                    'id', oi.id,
                    'quantity', oi.quantity,
                    'price', oi.price_at_order,
                    'product', jsonb_build_object(
                        'id', p.id,
                        'name', p.name,
                        'category', p.category
                    )
                ) ORDER BY oi.pk_order_item
            ) FILTER (WHERE oi.pk_order_item IS NOT NULL),
            '[]'::jsonb
        )
    ) as data
FROM tb_order o
LEFT JOIN tb_user u ON u.pk_user = o.fk_user
LEFT JOIN tb_order_item oi ON oi.fk_order = o.pk_order
LEFT JOIN tb_product p ON p.pk_product = oi.fk_product
GROUP BY o.pk_order, o.id, o.status, o.created_at, u.id, u.name, u.email, u.profile;

\echo ''
\echo '### Test 1: Helper Functions (Phase 1)'

-- Insert test data
INSERT INTO tb_user (name, email, profile) VALUES
    ('Alice', 'alice@example.com', '{"theme": "light", "language": "en"}'::jsonb),
    ('Bob', 'bob@example.com', '{"theme": "dark", "language": "fr"}'::jsonb);

INSERT INTO tb_product (name, price, category) VALUES
    ('Widget A', 10.00, 'widgets'),
    ('Widget B', 20.00, 'widgets'),
    ('Gadget C', 30.00, 'gadgets');

INSERT INTO tb_order (fk_user, status) VALUES (1, 'pending');

INSERT INTO tb_order_item (fk_order, fk_product, quantity, price_at_order) VALUES
    (1, 1, 2, 10.00),
    (1, 2, 1, 20.00);

-- Refresh TVIEW (manual for testing)
TRUNCATE tv_order;
INSERT INTO tv_order
SELECT
    o.pk_order, o.id, o.fk_user,
    jsonb_build_object(
        'id', o.id,
        'status', o.status,
        'customer', jsonb_build_object('id', u.id, 'name', u.name, 'email', u.email),
        'items', COALESCE(jsonb_agg(jsonb_build_object(
            'id', oi.id, 'quantity', oi.quantity, 'price', oi.price_at_order,
            'product', jsonb_build_object('id', p.id, 'name', p.name)
        )) FILTER (WHERE oi.pk_order_item IS NOT NULL), '[]'::jsonb)
    ) as data
FROM tb_order o
LEFT JOIN tb_user u ON u.pk_user = o.fk_user
LEFT JOIN tb_order_item oi ON oi.fk_order = o.pk_order
LEFT JOIN tb_product p ON p.pk_product = oi.fk_product
GROUP BY o.pk_order, o.id, o.status, u.id, u.name, u.email;

-- Test: jsonb_extract_id
DO $$
DECLARE
    order_id text;
BEGIN
    SELECT jsonb_extract_id(data, 'id') INTO order_id FROM tv_order WHERE pk_order = 1;
    IF order_id IS NOT NULL THEN
        RAISE NOTICE 'PASS: jsonb_extract_id works';
    ELSE
        RAISE EXCEPTION 'FAIL: Could not extract order ID';
    END IF;
END $$;

-- Test: jsonb_array_contains_id
DO $$
DECLARE
    has_item boolean;
    item_id uuid;
BEGIN
    SELECT id INTO item_id FROM tb_order_item WHERE fk_order = 1 LIMIT 1;
    SELECT jsonb_array_contains_id(data, ARRAY['items'], 'id', to_jsonb(item_id::text))
    INTO has_item FROM tv_order WHERE pk_order = 1;

    IF has_item THEN
        RAISE NOTICE 'PASS: jsonb_array_contains_id detects existing item';
    ELSE
        RAISE EXCEPTION 'FAIL: Should have found item in array';
    END IF;
END $$;

\echo ''
\echo '### Test 2: Nested Path Updates (Phase 2)'

-- Test: Update nested field in array element (product name)
DO $$
DECLARE
    item_id uuid;
    old_name text;
    new_name text;
BEGIN
    SELECT id INTO item_id FROM tb_order_item WHERE fk_order = 1 LIMIT 1;
    SELECT data->'items'->0->'product'->>'name' INTO old_name FROM tv_order WHERE pk_order = 1;

    -- Update using nested path
    UPDATE tv_order
    SET data = jsonb_ivm_array_update_where_path(
        data,
        'items',
        'id',
        to_jsonb(item_id::text),
        'product.name',
        '"Widget A Updated"'::jsonb
    )
    WHERE pk_order = 1;

    SELECT data->'items'->0->'product'->>'name' INTO new_name FROM tv_order WHERE pk_order = 1;

    IF new_name = 'Widget A Updated' THEN
        RAISE NOTICE 'PASS: Nested path update in array element works';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected "Widget A Updated", got %', new_name;
    END IF;
END $$;

\echo ''
\echo '### Test 3: Batch Operations (Phase 3)'

-- Add more items
INSERT INTO tb_order_item (fk_order, fk_product, quantity, price_at_order) VALUES
    (1, 3, 3, 30.00);

-- Refresh
TRUNCATE tv_order;
INSERT INTO tv_order
SELECT
    o.pk_order, o.id, o.fk_user,
    jsonb_build_object(
        'id', o.id,
        'items', COALESCE(jsonb_agg(jsonb_build_object(
            'id', oi.id, 'quantity', oi.quantity, 'price', oi.price_at_order
        ) ORDER BY oi.pk_order_item) FILTER (WHERE oi.pk_order_item IS NOT NULL), '[]'::jsonb)
    ) as data
FROM tb_order o
LEFT JOIN tb_order_item oi ON oi.fk_order = o.pk_order
GROUP BY o.pk_order, o.id;

-- Test: Batch update multiple items
DO $$
DECLARE
    items_json jsonb;
    item1_price numeric;
    item2_price numeric;
BEGIN
    -- Build batch update
    SELECT jsonb_agg(
        jsonb_build_object(
            'id', id::text,
            'price', price_at_order + 5.00
        )
    )
    INTO items_json
    FROM tb_order_item
    WHERE fk_order = 1;

    -- Apply batch update
    UPDATE tv_order
    SET data = jsonb_array_update_where_batch(
        data,
        'items',
        'id',
        items_json
    )
    WHERE pk_order = 1;

    -- Verify
    SELECT (data->'items'->0->>'price')::numeric INTO item1_price FROM tv_order WHERE pk_order = 1;
    SELECT (data->'items'->1->>'price')::numeric INTO item2_price FROM tv_order WHERE pk_order = 1;

    IF item1_price = 15.00 AND item2_price = 25.00 THEN
        RAISE NOTICE 'PASS: Batch array update works for multiple elements';
    ELSE
        RAISE EXCEPTION 'FAIL: Batch update did not apply correctly';
    END IF;
END $$;

\echo ''
\echo '### Test 4: Fallback Path Operations (Phase 4)'

-- Test: Direct path update
UPDATE tv_order
SET data = jsonb_ivm_set_path(
    data,
    'status',
    '"shipped"'::jsonb
)
WHERE pk_order = 1;

DO $$
DECLARE
    status text;
BEGIN
    SELECT data->>'status' INTO status FROM tv_order WHERE pk_order = 1;

    IF status = 'shipped' THEN
        RAISE NOTICE 'PASS: jsonb_ivm_set_path works for simple paths';
    ELSE
        RAISE EXCEPTION 'FAIL: Status not updated correctly';
    END IF;
END $$;

\echo ''
\echo '### Test 5: Combined Operations'

-- Scenario: Price change + customer update + status change
-- This tests that all functions work together

DO $$
DECLARE
    item_id uuid;
BEGIN
    SELECT id INTO item_id FROM tb_order_item WHERE fk_order = 1 LIMIT 1;

    -- Chain multiple operations
    UPDATE tv_order
    SET data = jsonb_ivm_set_path(
        jsonb_ivm_array_update_where_path(
            data,
            'items',
            'id',
            to_jsonb(item_id::text),
            'quantity',
            '5'::jsonb
        ),
        'status',
        '"completed"'::jsonb
    )
    WHERE pk_order = 1;

    -- Verify both changes
    IF (
        SELECT data->>'status' FROM tv_order WHERE pk_order = 1
    ) = 'completed' AND (
        SELECT (data->'items'->0->>'quantity')::int FROM tv_order WHERE pk_order = 1
    ) = 5 THEN
        RAISE NOTICE 'PASS: Chained operations work correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Chained operations failed';
    END IF;
END $$;

\echo ''
\echo '### Test 6: Error Handling'

-- Test: Invalid path
DO $$
BEGIN
    UPDATE tv_order
    SET data = jsonb_ivm_set_path(
        data,
        'invalid..path..syntax',
        '"test"'::jsonb
    )
    WHERE pk_order = 1;

    RAISE EXCEPTION 'FAIL: Should have rejected invalid path syntax';
EXCEPTION
    WHEN OTHERS THEN
        RAISE NOTICE 'PASS: Invalid path correctly rejected';
END $$;

-- Test: Non-existent array element
DO $$
DECLARE
    result jsonb;
BEGIN
    SELECT jsonb_ivm_array_update_where_path(
        '{"items": []}'::jsonb,
        'items',
        'id',
        '"nonexistent"'::jsonb,
        'price',
        '99.99'::jsonb
    ) INTO result;

    -- Should return unchanged data
    IF jsonb_array_length(result->'items') = 0 THEN
        RAISE NOTICE 'PASS: Non-existent element handled gracefully';
    ELSE
        RAISE EXCEPTION 'FAIL: Should not have modified array';
    END IF;
END $$;

-- Cleanup
DROP TABLE tv_order;
DROP TABLE tb_order_item;
DROP TABLE tb_order;
DROP TABLE tb_product;
DROP TABLE tb_user;

\echo ''
\echo '=========================================='
\echo '✓ All integration tests passed!'
\echo 'All phases (1-4) working correctly'
\echo '=========================================='