-- Test Event Trigger Phase 2
-- This test verifies that the event trigger now fully converts tables to TVIEWs

\set ECHO all

-- Setup test data
CREATE TABLE tb_test (id INT, name TEXT);
INSERT INTO tb_test VALUES (1, 'Alice'), (2, 'Bob');

-- Test 1: CREATE TABLE AS with TVIEW syntax (should now fully convert)
CREATE TABLE tv_test AS
SELECT id as pk_test, gen_random_uuid() as id,
       jsonb_build_object('id', id, 'name', name) as data
FROM tb_test;

-- Check that table was converted to TVIEW
SELECT 'Metadata registered' as test,
       EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'test') as passed;

-- Check that view was created
SELECT 'View created' as test,
       EXISTS(SELECT 1 FROM pg_views WHERE viewname = 'v_test') as passed;

-- Check that TVIEW wrapper exists
SELECT 'TVIEW wrapper exists' as test,
       EXISTS(SELECT 1 FROM pg_views WHERE viewname = 'tv_test') as passed;

-- Check data is accessible through TVIEW
SELECT 'Data accessible via TVIEW' as test, COUNT(*) = 2 as passed FROM tv_test;

-- Check that original table was dropped and replaced with view
SELECT 'Original table replaced with view' as test,
       (SELECT relkind FROM pg_class WHERE relname = 'tv_test') = 'v' as passed;

-- Test 2: Verify TVIEW structure
SELECT 'Has pk column' as test,
       EXISTS(SELECT 1 FROM information_schema.columns
              WHERE table_name = 'v_test' AND column_name = 'pk_test') as passed;

SELECT 'Has id column' as test,
       EXISTS(SELECT 1 FROM information_schema.columns
              WHERE table_name = 'v_test' AND column_name = 'id') as passed;

SELECT 'Has data column' as test,
       EXISTS(SELECT 1 FROM information_schema.columns
              WHERE table_name = 'v_test' AND column_name = 'data') as passed;

-- Cleanup
DROP TABLE tv_test CASCADE;
DROP TABLE tb_test;

SELECT 'Phase 2 test complete' as status;