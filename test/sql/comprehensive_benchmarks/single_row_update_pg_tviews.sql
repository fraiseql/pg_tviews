-- Simple benchmark test
-- ROWS: 1

-- Create test table
CREATE TABLE IF NOT EXISTS tb_benchmark_test (id SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE IF NOT EXISTS tv_benchmark_test AS SELECT id, data FROM tb_benchmark_test;
SELECT pg_tviews_convert_existing_table('tv_benchmark_test');

-- Insert test data
INSERT INTO tb_benchmark_test (data) VALUES ('test-data') ON CONFLICT DO NOTHING;