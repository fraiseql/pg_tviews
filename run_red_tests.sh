#!/bin/bash
echo "=== Running RED Phase Tests for Array Handling Implementation ==="
echo

echo "🧪 Test 1: 50_array_columns.sql - Array column materialization"
echo "-------------------------------------------------------------"
psql pg_tviews -f test/sql/50_array_columns.sql
echo

echo "🧪 Test 2: 51_jsonb_array_update.sql - JSONB array element updates"
echo "------------------------------------------------------------------"
psql pg_tviews -f test/sql/51_jsonb_array_update.sql
echo

echo "🧪 Test 3: 52_array_insert_delete.sql - Array INSERT/DELETE operations"
echo "-----------------------------------------------------------------------"
psql pg_tviews -f test/sql/52_array_insert_delete.sql
echo

echo "🧪 Test 4: 53_batch_optimization.sql - Batch update optimization"
echo "---------------------------------------------------------------"
psql pg_tviews -f test/sql/53_batch_optimization.sql
echo

echo "=== RED Phase Tests Complete ==="