-- Phase 2 Integration Tests: GUC Configuration System
-- Tests runtime configuration via GUC variables

-- Test 1: GUC variable availability and defaults
SELECT name, setting, short_desc
FROM pg_settings
WHERE name LIKE 'pg_tviews.%'
ORDER BY name;

-- Test 2: max_propagation_depth configuration
-- Create a scenario that would exceed default depth
CREATE TABLE test_guc_depth (
    pk_test_guc_depth BIGINT PRIMARY KEY,
    parent_id BIGINT REFERENCES test_guc_depth(pk_test_guc_depth)
);

-- Create a deeply nested hierarchy
INSERT INTO test_guc_depth VALUES (1, NULL);
INSERT INTO test_guc_depth VALUES (2, 1);
INSERT INTO test_guc_depth VALUES (3, 2);
INSERT INTO test_guc_depth VALUES (4, 3);
INSERT INTO test_guc_depth VALUES (5, 4);

SELECT pg_tviews_create('guc_depth', $$
    SELECT pk_test_guc_depth,
           jsonb_build_object(
               'parent_id', parent_id,
               'depth', (SELECT COUNT(*) FROM test_guc_depth t2
                        WHERE t2.pk_test_guc_depth <= t1.pk_test_guc_depth)
           ) as data
    FROM test_guc_depth t1
$$);

-- Test with default max_propagation_depth (100)
BEGIN;
    UPDATE test_guc_depth SET parent_id = parent_id WHERE pk_test_guc_depth = 1;
COMMIT;

-- Test with reduced max_propagation_depth (2)
SET pg_tviews.max_propagation_depth = 2;
BEGIN;
    -- This should succeed (within limit)
    UPDATE test_guc_depth SET parent_id = parent_id WHERE pk_test_guc_depth = 1;
COMMIT;

-- Test cache settings
-- Verify cache settings are respected
SHOW pg_tviews.graph_cache_enabled;
SHOW pg_tviews.table_cache_enabled;
SHOW pg_tviews.metrics_enabled;

-- Test cache disabling
SET pg_tviews.graph_cache_enabled = off;
SET pg_tviews.table_cache_enabled = off;
SET pg_tviews.metrics_enabled = on;

-- Verify settings changed
SHOW pg_tviews.graph_cache_enabled;
SHOW pg_tviews.table_cache_enabled;
SHOW pg_tviews.metrics_enabled;

-- Reset to defaults
RESET pg_tviews.max_propagation_depth;
RESET pg_tviews.graph_cache_enabled;
RESET pg_tviews.table_cache_enabled;
RESET pg_tviews.metrics_enabled;

-- Cleanup
DROP TABLE test_guc_depth CASCADE;
SELECT pg_tviews_drop('guc_depth');