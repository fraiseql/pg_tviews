-- cleanup_schema.sql
-- Simple schema-based cleanup for benchmarks

-- Drop and recreate the benchmark schema
-- This removes ALL benchmark objects in one command
DROP SCHEMA IF EXISTS benchmark CASCADE;
CREATE SCHEMA benchmark;
GRANT ALL ON SCHEMA benchmark TO postgres;

-- Set search path so benchmark objects are created in benchmark schema
SET search_path TO benchmark, public;

\echo '✓ Benchmark schema ready'
