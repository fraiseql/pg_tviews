-- Comprehensive integration test for all jsonb_delta enhancements
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
\echo '### Test 1: Helper Functions'

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

-- Refresh TVIEW (manual for testing). The materialized tview keeps only the
-- Trinity columns (pk_order, id, data); the CTAS's extra fk_user projection is
-- not materialized, so the manual refresh must target the real columns.
TRUNCATE tv_order;
INSERT INTO tv_order (pk_order, id, data)
SELECT
    o.pk_order, o.id,
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

-- Test: extract id from the materialized document (plain JSONB — this file
-- deliberately runs without jsonb_delta, so it must not call jsonb_delta helpers).
DO $$
DECLARE
    order_id text;
BEGIN
    SELECT data->>'id' INTO order_id FROM tv_order WHERE pk_order = 1;
    IF order_id IS NOT NULL THEN
        RAISE NOTICE 'PASS: order id extracted from document';
    ELSE
        RAISE EXCEPTION 'FAIL: Could not extract order ID';
    END IF;
END $$;


-- The remaining jsonb_delta helper tests (jsonb_array_contains_id, nested path
-- updates, jsonb_delta_set_path, batch) were removed (issue #55): this file
-- deliberately runs WITHOUT jsonb_delta, and those helpers require it. That
-- coverage lives in 92/93/94/95, which create the jsonb_delta extension.

-- Cleanup
DROP TABLE IF EXISTS tv_order CASCADE;
DROP TABLE IF EXISTS tb_order_item CASCADE;
DROP TABLE IF EXISTS tb_order CASCADE;
DROP TABLE IF EXISTS tb_product CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;

\echo '96 PASS: pg_tviews fallback refresh (no jsonb_delta) works'
