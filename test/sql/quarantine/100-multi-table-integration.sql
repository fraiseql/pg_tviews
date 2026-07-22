-- QUARANTINED (issue #55): exercises aggregate/window/summary TVIEWs (entity with
-- no tb_<entity> base table) which the #49 refreshability guard correctly rejects at
-- create time. Support for aggregate TVIEWs is tracked in #58; this test is excluded
-- from the CI regression suite until that lands, at which point it becomes acceptance
-- criteria (converted to the aggregate API/shape).
--
-- Multi-Table UNLOGGED TVIEW Integration Tests
-- Complex scenarios with cascading updates across multiple TVIEWs
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo '=========================================='
\echo 'Multi-Table UNLOGGED TVIEW Integration'
\echo 'Complex cascade scenarios'
\echo '=========================================='

CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Create a complex multi-entity e-commerce schema
CREATE TABLE mt_users (
    pk_user BIGINT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    balance DECIMAL(10,2) DEFAULT 0.00,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE mt_categories (
    pk_category BIGINT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    parent_category BIGINT REFERENCES mt_categories,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE mt_products (
    pk_product BIGINT PRIMARY KEY,
    fk_category BIGINT NOT NULL REFERENCES mt_categories,
    name TEXT NOT NULL,
    price DECIMAL(10,2) NOT NULL,
    stock_quantity INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE mt_orders (
    pk_order BIGINT PRIMARY KEY,
    fk_user BIGINT NOT NULL REFERENCES mt_users,
    status TEXT NOT NULL DEFAULT 'pending',
    total_amount DECIMAL(10,2) NOT NULL DEFAULT 0.00,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE mt_order_items (
    pk_item BIGINT PRIMARY KEY,
    fk_order BIGINT NOT NULL REFERENCES mt_orders,
    fk_product BIGINT NOT NULL REFERENCES mt_products,
    quantity INTEGER NOT NULL,
    unit_price DECIMAL(10,2) NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Insert comprehensive test data
INSERT INTO mt_users VALUES
    (1, 'alice@shop.com', 'Alice Johnson', 1000.00),
    (2, 'bob@shop.com', 'Bob Smith', 500.00),
    (3, 'carol@shop.com', 'Carol Williams', 750.00);

INSERT INTO mt_categories VALUES
    (10, 'Electronics', 'Electronic devices', NULL),
    (11, 'Laptops', 'Portable computers', 10),
    (12, 'Phones', 'Mobile devices', 10),
    (20, 'Books', 'Reading materials', NULL),
    (21, 'Fiction', 'Fiction books', 20);

INSERT INTO mt_products VALUES
    (100, 11, 'MacBook Pro', 2500.00, 50),
    (101, 11, 'Dell XPS', 1800.00, 30),
    (102, 12, 'iPhone 15', 1200.00, 100),
    (103, 12, 'Samsung Galaxy', 900.00, 75),
    (200, 21, 'The Great Gatsby', 15.99, 200),
    (201, 21, '1984', 12.99, 150);

INSERT INTO mt_orders VALUES
    (1000, 1, 'completed', 3820.00),
    (1001, 2, 'pending', 2715.99),
    (1002, 1, 'shipped', 15.99);

INSERT INTO mt_order_items VALUES
    (10000, 1000, 100, 1, 2500.00),
    (10001, 1000, 102, 1, 1200.00),
    (10002, 1000, 200, 1, 15.99),
    (10003, 1001, 101, 1, 1800.00),
    (10004, 1001, 103, 1, 900.00),
    (10005, 1001, 201, 1, 12.99),
    (10006, 1002, 200, 1, 15.99);

-- Create interconnected TVIEWs
\echo ''
\echo '### Creating Interconnected TVIEW Network'

-- User summary TVIEW
SELECT pg_tviews_create('mt_user_summary', $$
    SELECT
        u.pk_user as id,
        jsonb_build_object(
            'email', u.email,
            'name', u.name,
            'balance', u.balance,
            'order_stats', COALESCE(order_stats.stats, jsonb_build_object(
                'total_orders', 0,
                'total_spent', 0.00,
                'avg_order_value', 0.00
            )),
            'favorite_category', COALESCE(order_stats.fav_category, 'none')
        ) as data
    FROM mt_users u
    LEFT JOIN (
        SELECT
            o.fk_user,
            COUNT(DISTINCT o.pk_order) as total_orders,
            SUM(o.total_amount) as total_spent,
            AVG(o.total_amount) as avg_order_value,
            (
                SELECT c.name
                FROM mt_categories c
                JOIN mt_products p ON p.fk_category = c.pk_category
                JOIN mt_order_items oi ON oi.fk_product = p.pk_product
                JOIN mt_orders o2 ON o2.pk_order = oi.fk_order
                WHERE o2.fk_user = o.fk_user
                GROUP BY c.pk_category, c.name
                ORDER BY COUNT(*) DESC
                LIMIT 1
            ) as fav_category,
            jsonb_build_object(
                'total_orders', COUNT(DISTINCT o.pk_order),
                'total_spent', SUM(o.total_amount),
                'avg_order_value', AVG(o.total_amount)
            ) as stats
        FROM mt_orders o
        GROUP BY o.fk_user
    ) order_stats ON order_stats.fk_user = u.pk_user
$$);

-- Product summary TVIEW
SELECT pg_tviews_create('mt_product_summary', $$
    SELECT
        p.pk_product as id,
        jsonb_build_object(
            'name', p.name,
            'price', p.price,
            'stock_quantity', p.stock_quantity,
            'category', c.name,
            'sales_stats', COALESCE(sales.stats, jsonb_build_object(
                'total_sold', 0,
                'revenue', 0.00,
                'avg_quantity_per_order', 0.00
            ))
        ) as data
    FROM mt_products p
    JOIN mt_categories c ON c.pk_category = p.fk_category
    LEFT JOIN (
        SELECT
            oi.fk_product,
            SUM(oi.quantity) as total_sold,
            SUM(oi.quantity * oi.unit_price) as revenue,
            AVG(oi.quantity) as avg_quantity_per_order,
            jsonb_build_object(
                'total_sold', SUM(oi.quantity),
                'revenue', SUM(oi.quantity * oi.unit_price),
                'avg_quantity_per_order', AVG(oi.quantity)
            ) as stats
        FROM mt_order_items oi
        GROUP BY oi.fk_product
    ) sales ON sales.fk_product = p.pk_product
$$);

-- Category summary TVIEW
SELECT pg_tviews_create('mt_category_summary', $$
    SELECT
        c.pk_category as id,
        jsonb_build_object(
            'name', c.name,
            'description', c.description,
            'parent_name', COALESCE(parent.name, 'root'),
            'product_count', COALESCE(prod_stats.product_count, 0),
            'total_stock', COALESCE(prod_stats.total_stock, 0),
            'sales_performance', COALESCE(sales.performance, jsonb_build_object(
                'total_revenue', 0.00,
                'top_product', null
            ))
        ) as data
    FROM mt_categories c
    LEFT JOIN mt_categories parent ON parent.pk_category = c.parent_category
    LEFT JOIN (
        SELECT
            p.fk_category,
            COUNT(*) as product_count,
            SUM(p.stock_quantity) as total_stock
        FROM mt_products p
        GROUP BY p.fk_category
    ) prod_stats ON prod_stats.fk_category = c.pk_category
    LEFT JOIN (
        SELECT
            cat.pk_category,
            SUM(oi.quantity * oi.unit_price) as total_revenue,
            (
                SELECT jsonb_build_object('name', p.name, 'revenue', SUM(oi2.quantity * oi2.unit_price))
                FROM mt_order_items oi2
                JOIN mt_products p ON p.pk_product = oi2.fk_product
                WHERE p.fk_category = cat.pk_category
                GROUP BY p.pk_product, p.name
                ORDER BY SUM(oi2.quantity * oi2.unit_price) DESC
                LIMIT 1
            ) as top_product,
            jsonb_build_object(
                'total_revenue', SUM(oi.quantity * oi.unit_price),
                'top_product', (
                    SELECT jsonb_build_object('name', p.name, 'revenue', SUM(oi2.quantity * oi2.unit_price))
                    FROM mt_order_items oi2
                    JOIN mt_products p ON p.pk_product = oi2.fk_product
                    WHERE p.fk_category = cat.pk_category
                    GROUP BY p.pk_product, p.name
                    ORDER BY SUM(oi2.quantity * oi2.unit_price) DESC
                    LIMIT 1
                )
            ) as performance
        FROM mt_categories cat
        JOIN mt_products prod ON prod.fk_category = cat.pk_category
        LEFT JOIN mt_order_items oi ON oi.fk_product = prod.pk_product
        GROUP BY cat.pk_category
    ) sales ON sales.pk_category = c.pk_category
$$);

-- Verify all TVIEWs are UNLOGGED and have correct data
\echo ''
\echo '### Verifying TVIEW Creation and Data'

DO $$
DECLARE
    user_count INTEGER;
    product_count INTEGER;
    category_count INTEGER;
    all_unlogged BOOLEAN;
BEGIN
    -- Check row counts
    SELECT COUNT(*) INTO user_count FROM tv_mt_user_summary;
    SELECT COUNT(*) INTO product_count FROM tv_mt_product_summary;
    SELECT COUNT(*) INTO category_count FROM tv_mt_category_summary;

    IF user_count != 3 THEN
        RAISE EXCEPTION 'Expected 3 users, got %', user_count;
    END IF;

    IF product_count != 6 THEN
        RAISE EXCEPTION 'Expected 6 products, got %', product_count;
    END IF;

    IF category_count != 5 THEN
        RAISE EXCEPTION 'Expected 5 categories, got %', category_count;
    END IF;

    -- Check all are UNLOGGED
    SELECT COUNT(*) = 3 INTO all_unlogged
    FROM pg_class c
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE c.relname IN ('tv_mt_user_summary', 'tv_mt_product_summary', 'tv_mt_category_summary')
    AND n.nspname = 'public'
    AND c.relpersistence = 'u';

    IF NOT all_unlogged THEN
        RAISE EXCEPTION 'All TVIEWs should be UNLOGGED';
    END IF;

    RAISE NOTICE '✓ All TVIEWs created as UNLOGGED with correct data';
END $$;

-- Test complex cascade updates
\echo ''
\echo '### Testing Complex Cascade Updates'

-- Add a new order with multiple items
INSERT INTO mt_orders VALUES (1003, 3, 'completed', 0.00);

-- Add items to the new order (this should cascade to all TVIEWs)
INSERT INTO mt_order_items VALUES
    (10007, 1003, 100, 1, 2500.00),  -- MacBook Pro
    (10008, 1003, 103, 1, 900.00),   -- Samsung Galaxy
    (10009, 1003, 200, 2, 15.99);    -- 2 copies of Gatsby

-- Update order total
UPDATE mt_orders SET total_amount = 3431.98 WHERE pk_order = 1003;

-- Verify cascades propagated correctly
DO $$
DECLARE
    carol_data JSONB;
    macbook_data JSONB;
    electronics_data JSONB;
BEGIN
    -- Check Carol's user summary updated
    SELECT data INTO carol_data
    FROM tv_mt_user_summary
    WHERE id = 3;

    IF (carol_data->'order_stats'->>'total_orders')::INTEGER != 1 THEN
        RAISE EXCEPTION 'Carol should have 1 total order';
    END IF;

    -- Check MacBook sales updated
    SELECT data INTO macbook_data
    FROM tv_mt_product_summary
    WHERE id = 100;

    IF (macbook_data->'sales_stats'->>'total_sold')::INTEGER != 2 THEN
        RAISE EXCEPTION 'MacBook should have 2 total sold';
    END IF;

    -- Check Electronics category updated
    SELECT data INTO electronics_data
    FROM tv_mt_category_summary
    WHERE id = 10;

    IF (electronics_data->>'total_stock')::INTEGER != 255 THEN
        RAISE EXCEPTION 'Electronics should have 255 total stock';
    END IF;

    RAISE NOTICE '✓ Complex cascade updates propagated correctly across all TVIEWs';
END $$;

-- Test crash recovery across multiple TVIEWs
\echo ''
\echo '### Testing Multi-Table Crash Recovery'

-- Simulate crash by truncating all UNLOGGED TVIEWs
TRUNCATE TABLE tv_mt_user_summary, tv_mt_product_summary, tv_mt_category_summary;

-- Verify all are empty
DO $$
DECLARE
    user_count INTEGER;
    product_count INTEGER;
    category_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO user_count FROM tv_mt_user_summary;
    SELECT COUNT(*) INTO product_count FROM tv_mt_product_summary;
    SELECT COUNT(*) INTO category_count FROM tv_mt_category_summary;

    IF user_count + product_count + category_count != 0 THEN
        RAISE EXCEPTION 'All TVIEWs should be empty after truncate';
    END IF;

    RAISE NOTICE '✓ All TVIEWs truncated (crash simulated)';
END $$;

-- Recover all TVIEWs
SELECT pg_tviews_recover_after_crash('mt_user_summary');
SELECT pg_tviews_recover_after_crash('mt_product_summary');
SELECT pg_tviews_recover_after_crash('mt_category_summary');

-- Verify recovery
DO $$
DECLARE
    user_count INTEGER;
    product_count INTEGER;
    category_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO user_count FROM tv_mt_user_summary;
    SELECT COUNT(*) INTO product_count FROM tv_mt_product_summary;
    SELECT COUNT(*) INTO category_count FROM tv_mt_category_summary;

    IF user_count != 3 OR product_count != 6 OR category_count != 5 THEN
        RAISE EXCEPTION 'Recovery failed: users=%, products=%, categories=%', user_count, product_count, category_count;
    END IF;

    RAISE NOTICE '✓ All TVIEWs recovered successfully';
END $$;

-- Clean up
SELECT pg_tviews_drop('mt_user_summary');
SELECT pg_tviews_drop('mt_product_summary');
SELECT pg_tviews_drop('mt_category_summary');

\echo ''
\echo '=========================================='
\echo '✅ Multi-table UNLOGGED TVIEW integration tests passed!'
\echo '=========================================='