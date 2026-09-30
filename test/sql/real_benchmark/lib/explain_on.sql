-- Turn on auto_explain for this session so every statement pg_tviews runs during
-- a refresh flush (nested SPI statements included) is reported with
-- ANALYZE + BUFFERS + WAL as a JSON plan. log_level = notice sends the plans to
-- the client, so no server-log access is needed; extract them with
-- lib/explain_extract.py. auto_explain perturbs timings: use it only in the
-- separate explain pass, never in a timed run.
LOAD 'auto_explain';
SET auto_explain.log_min_duration = 0;
SET auto_explain.log_analyze = on;
SET auto_explain.log_buffers = on;
SET auto_explain.log_wal = on;
SET auto_explain.log_timing = on;
SET auto_explain.log_nested_statements = on;
SET auto_explain.log_format = json;
SET auto_explain.log_level = notice;
SET client_min_messages = notice;
