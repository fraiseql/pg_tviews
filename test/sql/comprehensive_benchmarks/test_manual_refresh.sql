-- Basic Test for Manual Refresh Function
-- Test the core functionality before running full benchmarks

-- Create test database if it doesn't exist
SELECT 'Creating test database...' as status;
-- Note: Database creation is handled by run_benchmarks.sh

-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
-- Note: gen_random_uuid() is available in uuid-ossp extension

-- Load jsonb_ivm stubs if real extension not available
DO $$
BEGIN
    -- Try to create extension
    CREATE EXTENSION IF NOT EXISTS jsonb_ivm;
    RAISE NOTICE '✓ Using REAL jsonb_ivm extension';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE '⚠ jsonb_ivm extension not available, loading stubs';
    -- Load stubs would go here
END $$;

-- Load the refresh function
\i functions/refresh_product_manual.sql

-- Create minimal test schema (matching the actual implementation)
CREATE TABLE manual_func_product (
    pk_product INTEGER PRIMARY KEY,
    fk_category INTEGER DEFAULT 1,
    data JSONB DEFAULT '{}',
    version INTEGER DEFAULT 1,
    updated_at TIMESTAMPTZ DEFAULT now()
);

-- Create supporting tables for cascade testing
CREATE TABLE tb_product (
    pk_product INTEGER PRIMARY KEY,
    fk_category INTEGER DEFAULT 1,
    name TEXT,
    current_price NUMERIC(10,2),
    base_price NUMERIC(10,2)
);

CREATE TABLE tb_category (
    id UUID DEFAULT uuid_generate_v4(),
    pk_category INTEGER PRIMARY KEY,
    name TEXT DEFAULT 'Test Category',
    slug TEXT DEFAULT 'test-category'
);

CREATE TABLE tb_supplier (
    pk_supplier INTEGER PRIMARY KEY,
    name TEXT DEFAULT 'Test Supplier'
);

-- Insert test data
INSERT INTO tb_product (pk_product, name, current_price, base_price)
VALUES
    (1, 'Test Product 1', 100.00, 120.00),
    (2, 'Test Product 2', 200.00, 250.00);

INSERT INTO tb_category (pk_category, name) VALUES (1, 'Electronics');

INSERT INTO manual_func_product (pk_product, fk_category, data)
VALUES
    (1, 1, '{"id": "uuid-1", "pk": 1, "name": "Test Product 1", "price": {"current": 100.00, "base": 120.00, "discount_pct": 16.67}}'),
    (2, 1, '{"id": "uuid-2", "pk": 2, "name": "Test Product 2", "price": {"current": 200.00, "base": 250.00, "discount_pct": 20.00}}');

-- Test 1: Single product refresh
SELECT 'Test 1: Single product refresh' as test_name;
UPDATE tb_product SET current_price = 90.00 WHERE pk_product = 1;
SELECT refresh_product_manual('product', 1, 'price_current');

-- Verify the update worked
SELECT
    p.pk_product,
    p.current_price as source_price,
    m.data->'price'->'current' as materialized_price,
    m.data->'price'->'discount_pct' as discount_pct
FROM tb_product p
JOIN manual_func_product m ON p.pk_product = m.pk_product
WHERE p.pk_product = 1;

-- Test 2: Check return value structure
SELECT 'Test 2: Return value structure' as test_name;
SELECT refresh_product_manual('product', 2, 'price_current');

-- Test 3: Category cascade test
SELECT 'Test 3: Category cascade' as test_name;
UPDATE tb_category SET name = 'Updated Electronics' WHERE pk_category = 1;
SELECT refresh_product_manual('category', 1, 'name');

-- Verify cascade worked
SELECT
    c.name as category_name,
    m.data->'category'->'name' as materialized_category
FROM tb_category c
JOIN manual_func_product m ON c.pk_category = m.fk_category
WHERE c.pk_category = 1;

-- Test 4: Invalid entity type
SELECT 'Test 4: Invalid entity type (should show error)' as test_name;
SELECT refresh_product_manual('invalid_entity', 1, 'test');

-- Test 5: Non-existent product
SELECT 'Test 5: Non-existent product (should show error)' as test_name;
SELECT refresh_product_manual('product', 999, 'test');

SELECT 'Basic tests completed. Check results above.' as status;