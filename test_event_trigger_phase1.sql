-- Test Event Trigger Phase 1
-- This test verifies that the event trigger infrastructure is working

\set ECHO all

-- Setup test data
CREATE TABLE tb_test (id INT, name TEXT);
INSERT INTO tb_test VALUES (1, 'Alice'), (2, 'Bob');

-- Test 1: CREATE TABLE AS with TVIEW syntax (should trigger event)
CREATE TABLE tv_test AS
SELECT id as pk_test, id, jsonb_build_object('id', id, 'name', name) as data
FROM tb_test;

-- Check that table exists (event trigger should have fired but not converted yet)
SELECT 'Table exists' as test, EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = 'tv_test') as passed;

-- Check table structure (should be regular table, not TVIEW yet)
SELECT 'Has pk column' as test,
       EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name = 'tv_test' AND column_name = 'pk_test') as passed;

SELECT 'Has id column' as test,
       EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name = 'tv_test' AND column_name = 'id') as passed;

SELECT 'Has data column' as test,
       EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name = 'tv_test' AND column_name = 'data') as passed;

-- Check that it's NOT a TVIEW yet (no metadata)
SELECT 'No metadata yet' as test,
       NOT EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'test') as passed;

-- Check data is there
SELECT 'Data count' as test, COUNT(*) = 2 as passed FROM tv_test;

-- Cleanup
DROP TABLE tv_test;
DROP TABLE tb_test;

SELECT 'Phase 1 test complete' as status;