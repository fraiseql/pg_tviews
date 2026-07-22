-- Standalone preamble (issue #55): create the extensions and load the shared
-- security helpers (assert_rejects_injection / assert_accepts_valid) so this file
-- passes on its own in a fresh database under psql -v ON_ERROR_STOP=1.
\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir 00-security-test-helpers.sql
-- Comprehensive Security Test Suite
-- Tests all phases for SQL injection vulnerabilities

\echo '=========================================='
\echo 'Comprehensive Security Test Suite'
\echo 'Tests all phases for SQL injection'
\echo '=========================================='

-- SQL-injection rejection for the public pg_tviews functions that take
-- identifier arguments (issue #55: the previous targets — extract_jsonb_id,
-- update_array_element_path, … — are not part of the current API). Each
-- malicious identifier must be rejected (validated, or treated as a literal via
-- a parameterized lookup) and never executed.
\echo '### Identifier validation'
SELECT assert_rejects_injection(
    'pg_tviews_create: tview_name injection',
    $$SELECT pg_tviews_create('tv_x; DROP TABLE tb_x; --', 'SELECT 1')$$
);
SELECT assert_rejects_injection(
    'pg_tviews_create: quote-escape injection',
    $$SELECT pg_tviews_create('tv_x''; DROP TABLE tb_x; --', 'SELECT 1')$$
);

\echo '### Drop / convert'
SELECT assert_rejects_injection(
    'pg_tviews_drop: tview_name injection',
    $$SELECT pg_tviews_drop('tv_x''; DROP TABLE y; --')$$
);
SELECT assert_rejects_injection(
    'pg_tviews_convert_existing_table: name injection',
    $$SELECT pg_tviews_convert_existing_table('tv_x; DROP TABLE y; --')$$
);

-- Entity lookups are parameterized, so a malicious entity is treated as a
-- (non-existent) literal rather than executed.
\echo '### Refresh (parameterized entity lookup)'
SELECT assert_rejects_injection(
    'pg_tviews_refresh: entity injection',
    $$SELECT pg_tviews_refresh('x; DROP TABLE y; --')$$,
    'not found|invalid|injection|security'
);

\echo '### All security tests passed! ✓'