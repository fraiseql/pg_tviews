-- Multi-Hop Cascade Integration Tests (Issue #006)
-- Tests trigger-time cascade through non-TVIEW intermediate tables
-- e.g., tb_item changes cascade through tb_group to tv_order
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo '=========================================='
\echo 'Multi-Hop Cascade Integration Tests'
\echo 'Trigger-time cascade through intermediate tables'
\echo '=========================================='

CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Schema: tb_order -> tb_group -> tb_item
-- tv_order JOINs all three, so a change to tb_item must cascade
-- through tb_group to find the affected order PK.

CREATE TABLE tb_order (
    pk_order BIGINT PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    customer TEXT NOT NULL
);

CREATE TABLE tb_group (
    pk_group BIGINT PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_order BIGINT NOT NULL REFERENCES tb_order,
    label TEXT NOT NULL
);

CREATE TABLE tb_item (
    pk_item BIGINT PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_group BIGINT NOT NULL REFERENCES tb_group,
    product TEXT NOT NULL,
    qty INTEGER NOT NULL DEFAULT 1
);

-- Seed data: 2 orders, 3 groups, 5 items
INSERT INTO tb_order (pk_order, customer) VALUES (1, 'Alice'), (2, 'Bob');
INSERT INTO tb_group (pk_group, fk_order, label) VALUES (10, 1, 'Group A1'), (20, 1, 'Group A2'), (30, 2, 'Group B1');
INSERT INTO tb_item (pk_item, fk_group, product, qty) VALUES
    (100, 10, 'Widget', 2), (101, 10, 'Gadget', 1),
    (102, 20, 'Bolt', 5),
    (103, 30, 'Nut', 10), (104, 30, 'Washer', 3);

-- Create a TVIEW on tb_order that JOINs through tb_group to tb_item
SELECT pg_tviews_create('order', $$
    SELECT
        o.pk_order,
        o.id,
        jsonb_build_object(
            'customer', o.customer,
            'items', COALESCE(jsonb_agg(
                jsonb_build_object(
                    'product', i.product,
                    'qty', i.qty,
                    'group', g.label
                )
            ) FILTER (WHERE i.pk_item IS NOT NULL), '[]'::jsonb)
        ) AS data
    FROM tb_order o
    LEFT JOIN tb_group g ON g.fk_order = o.pk_order
    LEFT JOIN tb_item i ON i.fk_group = g.pk_group
    GROUP BY o.pk_order, o.id, o.customer
$$);

-- Verify cascade_paths were stored
\echo ''
\echo '### Test 1: cascade_paths stored in pg_tview_meta'

DO $$
DECLARE
    path_count INTEGER;
BEGIN
    SELECT array_length(cascade_paths, 1)
    INTO path_count
    FROM pg_tview_meta
    WHERE entity = 'order';

    IF path_count IS NULL OR path_count < 1 THEN
        RAISE EXCEPTION 'Expected at least 1 cascade path, got %', COALESCE(path_count::text, 'NULL');
    END IF;

    RAISE NOTICE 'cascade_paths count: %', path_count;
END $$;

-- Verify initial data
\echo ''
\echo '### Test 2: Initial population'

DO $$
DECLARE
    row_count INTEGER;
    alice_items INTEGER;
BEGIN
    SELECT COUNT(*) INTO row_count FROM tv_order;
    IF row_count != 2 THEN
        RAISE EXCEPTION 'Expected 2 orders, got %', row_count;
    END IF;

    SELECT (jsonb_array_length(data->'items'))
    INTO alice_items
    FROM tv_order WHERE pk_order = 1;

    IF alice_items != 3 THEN
        RAISE EXCEPTION 'Expected 3 items for Alice, got %', alice_items;
    END IF;

    RAISE NOTICE 'Initial population correct: 2 orders, Alice has 3 items';
END $$;

-- Test 3: UPDATE on leaf table (tb_item) cascades to tv_order
\echo ''
\echo '### Test 3: UPDATE on leaf table cascades through 2 hops'

UPDATE tb_item SET product = 'SuperWidget', qty = 99 WHERE pk_item = 100;

DO $$
DECLARE
    item_data JSONB;
    found BOOLEAN := false;
    elem JSONB;
BEGIN
    SELECT data->'items' INTO item_data FROM tv_order WHERE pk_order = 1;

    FOR elem IN SELECT jsonb_array_elements(item_data)
    LOOP
        IF elem->>'product' = 'SuperWidget' AND (elem->>'qty')::int = 99 THEN
            found := true;
        END IF;
    END LOOP;

    IF NOT found THEN
        RAISE EXCEPTION 'Item update did not cascade: expected SuperWidget with qty=99 in order 1, got %', item_data;
    END IF;

    RAISE NOTICE 'Leaf UPDATE cascaded correctly through 2 hops';
END $$;

-- Test 4: INSERT on leaf table cascades
\echo ''
\echo '### Test 4: INSERT on leaf table cascades'

INSERT INTO tb_item (pk_item, fk_group, product, qty) VALUES (105, 20, 'Spring', 7);

DO $$
DECLARE
    alice_items INTEGER;
BEGIN
    SELECT jsonb_array_length(data->'items')
    INTO alice_items
    FROM tv_order WHERE pk_order = 1;

    IF alice_items != 4 THEN
        RAISE EXCEPTION 'Expected 4 items for Alice after INSERT, got %', alice_items;
    END IF;

    RAISE NOTICE 'Leaf INSERT cascaded: Alice now has 4 items';
END $$;

-- Test 5: DELETE on leaf table cascades
\echo ''
\echo '### Test 5: DELETE on leaf table cascades'

DELETE FROM tb_item WHERE pk_item = 105;

DO $$
DECLARE
    alice_items INTEGER;
BEGIN
    SELECT jsonb_array_length(data->'items')
    INTO alice_items
    FROM tv_order WHERE pk_order = 1;

    IF alice_items != 3 THEN
        RAISE EXCEPTION 'Expected 3 items for Alice after DELETE, got %', alice_items;
    END IF;

    RAISE NOTICE 'Leaf DELETE cascaded: Alice back to 3 items';
END $$;

-- Test 6: UPDATE on intermediate table (tb_group) cascades
\echo ''
\echo '### Test 6: UPDATE on intermediate table cascades'

UPDATE tb_group SET label = 'Updated Group A1' WHERE pk_group = 10;

DO $$
DECLARE
    item_data JSONB;
    found BOOLEAN := false;
    elem JSONB;
BEGIN
    SELECT data->'items' INTO item_data FROM tv_order WHERE pk_order = 1;

    FOR elem IN SELECT jsonb_array_elements(item_data)
    LOOP
        IF elem->>'group' = 'Updated Group A1' THEN
            found := true;
        END IF;
    END LOOP;

    IF NOT found THEN
        RAISE EXCEPTION 'Group update did not cascade to tv_order';
    END IF;

    RAISE NOTICE 'Intermediate UPDATE cascaded correctly';
END $$;

-- Test 7: Bulk UPDATE on leaf table
\echo ''
\echo '### Test 7: Bulk UPDATE on leaf table'

UPDATE tb_item SET qty = qty + 100 WHERE fk_group = 30;

DO $$
DECLARE
    bob_data JSONB;
    elem JSONB;
    total_qty INTEGER := 0;
BEGIN
    SELECT data->'items' INTO bob_data FROM tv_order WHERE pk_order = 2;

    FOR elem IN SELECT jsonb_array_elements(bob_data)
    LOOP
        total_qty := total_qty + (elem->>'qty')::int;
    END LOOP;

    -- Original: Nut=10, Washer=3 -> after +100: Nut=110, Washer=103 -> total=213
    IF total_qty != 213 THEN
        RAISE EXCEPTION 'Bulk update: expected total qty 213, got %', total_qty;
    END IF;

    RAISE NOTICE 'Bulk UPDATE cascaded correctly: total qty = 213';
END $$;

-- Test 8: NULL FK handling -- item with NULL fk_group should not crash
\echo ''
\echo '### Test 8: NULL FK handling'

-- Temporarily drop the NOT NULL constraint for this test
ALTER TABLE tb_item ALTER COLUMN fk_group DROP NOT NULL;

INSERT INTO tb_item (pk_item, fk_group, product, qty) VALUES (200, NULL, 'Orphan', 1);

-- Should not crash, the cascade should silently skip this row
DO $$
BEGIN
    RAISE NOTICE 'NULL FK handled gracefully (no crash)';
END $$;

-- Clean up
DELETE FROM tb_item WHERE pk_item = 200;
ALTER TABLE tb_item ALTER COLUMN fk_group SET NOT NULL;

-- Test 9: Verify order 2 was not affected by order 1 changes
\echo ''
\echo '### Test 9: Cross-order isolation'

DO $$
DECLARE
    bob_customer TEXT;
BEGIN
    SELECT data->>'customer' INTO bob_customer FROM tv_order WHERE pk_order = 2;

    IF bob_customer != 'Bob' THEN
        RAISE EXCEPTION 'Order 2 customer corrupted: expected Bob, got %', bob_customer;
    END IF;

    RAISE NOTICE 'Cross-order isolation verified';
END $$;

-- Cleanup
\echo ''
\echo '### Cleanup'

SELECT pg_tviews_drop('order');
DROP TABLE tb_item CASCADE;
DROP TABLE tb_group CASCADE;
DROP TABLE tb_order CASCADE;

\echo ''
\echo '=========================================='
\echo 'All multi-hop cascade tests passed'
\echo '=========================================='
