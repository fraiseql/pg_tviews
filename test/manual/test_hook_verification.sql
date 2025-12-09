-- Complete test of CREATE and DROP TABLE hooks for tv_* tables
\echo '====== STEP 1: Clean slate ======'
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP TABLE IF EXISTS tb_product CASCADE;
DROP TABLE IF EXISTS tv_product CASCADE;

\echo '====== STEP 2: Create base table ======'
CREATE TABLE tb_product (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    price NUMERIC(10,2)
);

INSERT INTO tb_product (name, price) VALUES
    ('Widget', 19.99),
    ('Gadget', 29.99),
    ('Gizmo', 39.99);

\echo '====== STEP 3: Load extension (creates pg_tview_meta table) ======'
CREATE EXTENSION pg_tviews;

\echo '====== STEP 4: Verify metadata table exists ======'
\d pg_tview_meta

\echo '====== STEP 5: CREATE TABLE tv_product via HOOK ======'
-- This should be intercepted by the hook and converted to TVIEW
CREATE TABLE tv_product AS
SELECT id AS pk_product, name, price FROM tb_product;

\echo '====== STEP 6: Verify TVIEW was created ======'
SELECT * FROM v_product ORDER BY pk_product;
SELECT entity, view_name, table_name FROM pg_tview_meta WHERE entity = 'product';

\echo '====== STEP 7: DROP TABLE tv_product via HOOK ======'
-- This should be intercepted by the hook and clean up everything
DROP TABLE tv_product CASCADE;

\echo '====== STEP 8: Verify TVIEW was dropped ======'
SELECT COUNT(*) AS "metadata_count (should be 0)" FROM pg_tview_meta WHERE entity = 'product';

-- Try to query the view (should fail)
\set ON_ERROR_STOP off
SELECT * FROM v_product;
\set ON_ERROR_STOP on

\echo '====== ✅ SUCCESS! Both hooks are working! ======'
