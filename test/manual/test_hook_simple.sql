-- Test if the hook is actually being called for any DDL
-- Looking for "🔧 HOOK CALLED" messages in the logs

\echo '====== TEST 1: Simple CREATE TABLE ======'
DROP TABLE IF EXISTS test_simple CASCADE;
CREATE TABLE test_simple (id INT);

\echo '====== TEST 2: Create TV_* table ======'
DROP TABLE IF EXISTS tv_simple CASCADE;
CREATE TABLE tv_simple (id INT);

\echo '====== TEST 3: DROP TABLE ======'
DROP TABLE test_simple;

\echo '====== TEST 4: DROP TABLE tv_* ======'
DROP TABLE IF EXISTS tv_simple CASCADE;

\echo '====== Check PostgreSQL logs now! ======'
\echo 'Run: tail -50 ~/.pgrx/17.log | grep "🔧"'
