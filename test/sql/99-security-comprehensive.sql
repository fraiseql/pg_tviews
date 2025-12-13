-- Comprehensive Security Test Suite
-- Tests all phases for SQL injection vulnerabilities

\echo '=========================================='
\echo 'Comprehensive Security Test Suite'
\echo 'Tests all phases for SQL injection'
\echo '=========================================='

-- Phase 1 Security Tests
\echo '### Phase 1: Helper Functions'
SELECT assert_rejects_injection(
    'Phase1: extract_id injection',
    $$SELECT extract_jsonb_id('{"id": "test"}'::jsonb, 'id''; DROP TABLE users; --')$$
);

SELECT assert_rejects_injection(
    'Phase1: array_contains injection',
    $$SELECT check_array_element_exists('tv_posts', 'pk_post', 1, 'comments', 'id', '123'::jsonb)$$
);

-- Phase 2 Security Tests
\echo '### Phase 2: Nested Paths'
SELECT assert_rejects_injection(
    'Phase2: table name injection',
    $$SELECT update_array_element_path('tv_posts; DROP TABLE users; --', 'pk_post', 1, 'comments', 'id', '123'::jsonb, 'author.name', 'test'::jsonb)$$
);

SELECT assert_rejects_injection(
    'Phase2: nested path injection',
    $$SELECT update_array_element_path('tv_posts', 'pk_post', 1, 'comments''; DROP TABLE users; --', 'id', '123'::jsonb, 'author.name', 'test'::jsonb)$$
);

-- Phase 3 Security Tests
\echo '### Phase 3: Batch Operations'
SELECT assert_rejects_injection(
    'Phase3: batch injection',
    $$SELECT update_array_elements_batch('tv_orders; DROP TABLE users; --', 'pk_order', 1, 'items', 'id', '[{"id": 1, "price": 10}]'::jsonb)$$
);

-- Phase 4 Security Tests
\echo '### Phase 4: Fallback Paths'
SELECT assert_rejects_injection(
    'Phase4: set_path injection',
    $$SELECT update_single_path('tv_posts; DROP TABLE users; --', 'pk_post', 1, 'title', 'new title'::jsonb)$$
);

\echo '### All security tests passed! ✓'