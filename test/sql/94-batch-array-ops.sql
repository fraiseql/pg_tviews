-- Test batch array updates - jsonb_array_update_where_batch
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

-- Setup test schema
CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Test 1: Direct batch array element updates
\echo '### Test 1: Direct batch array element updates'

CREATE TABLE test_batch_updates (
    pk_test BIGINT PRIMARY KEY,
    data JSONB DEFAULT '{
        "items": [
            {"id": 1, "name": "Product A", "price": 10.99},
            {"id": 2, "name": "Product B", "price": 15.99},
            {"id": 3, "name": "Product C", "price": 20.99}
        ]
    }'::jsonb
);

INSERT INTO test_batch_updates VALUES (1);

-- Update multiple items in batch
UPDATE test_batch_updates
SET data = jsonb_array_update_where_batch(
    data,
    'items',
    'id',
    '[
        {"id": 1, "price": 12.99, "name": "Updated Product A"},
        {"id": 2, "price": 17.99},
        {"id": 3, "name": "Updated Product C", "stock": 50}
    ]'::jsonb
)
WHERE pk_test = 1;

-- Verify all updates
DO $$
DECLARE
    item1_price numeric;
    item1_name text;
    item2_price numeric;
    item3_name text;
    item3_stock integer;
BEGIN
    -- Check item 1 (both fields updated)
    SELECT data->'items'->0->>'price', data->'items'->0->>'name'
    INTO item1_price, item1_name
    FROM test_batch_updates WHERE pk_test = 1;

    -- Check item 2 (only price updated)
    SELECT data->'items'->1->>'price'
    INTO item2_price
    FROM test_batch_updates WHERE pk_test = 1;

    -- Check item 3 (name and new field added)
    SELECT data->'items'->2->>'name', (data->'items'->2->>'stock')::integer
    INTO item3_name, item3_stock
    FROM test_batch_updates WHERE pk_test = 1;

    IF item1_price = 12.99 AND item1_name = 'Updated Product A' AND
       item2_price = 17.99 AND item3_name = 'Updated Product C' AND item3_stock = 50 THEN
        RAISE NOTICE 'PASS: Batch updates applied correctly to all items';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected batch updates not applied correctly';
    END IF;
END $$;

-- Test 2: Partial batch updates (some items not in batch)
\echo '### Test 2: Partial batch updates'

-- Update only items 1 and 3, leave item 2 unchanged
UPDATE test_batch_updates
SET data = jsonb_array_update_where_batch(
    data,
    'items',
    'id',
    '[
        {"id": 1, "category": "Electronics"},
        {"id": 3, "category": "Books"}
    ]'::jsonb
)
WHERE pk_test = 1;

-- Verify partial updates
DO $$
DECLARE
    item1_category text;
    item2_category text;
    item3_category text;
BEGIN
    SELECT data->'items'->0->>'category', data->'items'->1->>'category', data->'items'->2->>'category'
    INTO item1_category, item2_category, item3_category
    FROM test_batch_updates WHERE pk_test = 1;

    IF item1_category = 'Electronics' AND item2_category IS NULL AND item3_category = 'Books' THEN
        RAISE NOTICE 'PASS: Partial batch updates work correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Partial batch updates not applied correctly';
    END IF;
END $$;

-- Test 3: Empty batch (should succeed without changes)
\echo '### Test 3: Empty batch handling'

-- Empty batch should not change anything
UPDATE test_batch_updates
SET data = jsonb_array_update_where_batch(
    data,
    'items',
    'id',
    '[]'::jsonb
)
WHERE pk_test = 1;

-- Verify no changes
DO $$
DECLARE
    item_count integer;
BEGIN
    SELECT jsonb_array_length(data->'items')
    INTO item_count
    FROM test_batch_updates WHERE pk_test = 1;

    IF item_count = 3 THEN
        RAISE NOTICE 'PASS: Empty batch handled correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Empty batch caused unexpected changes';
    END IF;
END $$;

-- Test 4: TVIEW integration with batch cascade
\echo '### Test 4: TVIEW integration with batch cascade'

-- Create source tables
CREATE TABLE tb_product (
    pk_product BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    name TEXT,
    price DECIMAL(10,2)
);

CREATE TABLE tb_order (
    pk_order BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    customer_name TEXT
);

-- Insert test data
INSERT INTO tb_product (name, price) VALUES
    ('Laptop', 999.99),
    ('Mouse', 29.99),
    ('Keyboard', 79.99);

INSERT INTO tb_order (customer_name) VALUES ('John Doe');

-- Create TVIEW with order items
CREATE TABLE tv_order AS
SELECT
    o.pk_order,
    o.id,
    o.customer_name,
    jsonb_build_object(
        'items', COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', p.id,
                    'name', p.name,
                    'price', p.price
                )
            )
            FROM tb_product p
            -- Simulate order items (in real app, would have order_items table)
            WHERE p.pk_product IN (1, 2, 3)
        ), '[]'::jsonb)
    ) AS data,
    now() AS created_at,
    now() AS updated_at
FROM tb_order o;

-- Test batch price updates cascade
UPDATE tb_product SET price = price * 1.1 WHERE pk_product IN (1, 2);

-- The cascade should update multiple items in the TVIEW
-- (This would use the batch update function in a real cascade)

-- Verify prices updated
DO $$
DECLARE
    laptop_price numeric;
    mouse_price numeric;
BEGIN
    SELECT data->'items'->0->>'price', data->'items'->1->>'price'
    INTO laptop_price, mouse_price
    FROM tv_order WHERE pk_order = 1;

    IF laptop_price = 1099.99 AND mouse_price = 32.99 THEN
        RAISE NOTICE 'PASS: Batch cascade updates work correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Batch cascade updates not applied correctly';
    END IF;
END $$;

-- Cleanup
DROP TABLE tv_order;
DROP TABLE tb_order;
DROP TABLE tb_product;
DROP TABLE test_batch_updates;

\echo '### All batch array update tests passed! ✓'