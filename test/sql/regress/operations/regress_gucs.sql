-- The settings (ADR 0220): five a
-- user decides; hidden and superuser-only, three diagnostic switches and the queue
-- bound (kept so its error can be tested); nothing else. Every other
-- limit is a constant named by its error. A removed or misspelled pg_tviews.*
-- name is refused, not silently ignored.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_gucs.sql
-- expect-output: gucs: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'gucs FAIL: %', what; END IF; END $$;
-- The SQLSTATE `stmt` fails with, or 'ok'.
CREATE FUNCTION outcome(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'ok';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE; END $$;

-- 1. The user settings: exactly these, each superuser-set (or postmaster).
SELECT must((SELECT string_agg(name || '=' || context, ', ' ORDER BY name)
               FROM pg_settings WHERE name LIKE 'pg_tviews.%')
            = 'pg_tviews.audit_enabled=superuser, '
              'pg_tviews.auto_rebuild_databases=postmaster, '
              'pg_tviews.lock_escalation_threshold=superuser, '
              'pg_tviews.max_refresh_rows=superuser, '
              'pg_tviews.publish_changes=superuser',
            'visible settings: ' || (SELECT string_agg(name || '=' || context, ', ' ORDER BY name)
                                       FROM pg_settings WHERE name LIKE 'pg_tviews.%'));

-- 2. The diagnostic switches and the queue bound exist, are hidden from SHOW ALL
--    and pg_settings, and only a superuser sets them (the test switch exists in
--    test builds only).
SELECT must(current_setting(n, true) = 'on' AND n NOT IN (SELECT name FROM pg_settings),
            n || ' missing or shown')
  FROM unnest(ARRAY['pg_tviews.graph_cache_enabled', 'pg_tviews.table_cache_enabled',
                    'pg_tviews.direct_patch_enabled']) n;
SELECT must(current_setting('pg_tviews.max_queue_size', true) = '1000000'
            AND 'pg_tviews.max_queue_size' NOT IN (SELECT name FROM pg_settings),
            'max_queue_size missing or shown');
DROP ROLE IF EXISTS regress_gucs_user;
CREATE ROLE regress_gucs_user;
SET ROLE regress_gucs_user;
SELECT must(outcome(format('SET %s = off', n)) = '42501', n || ' settable by a user')
  FROM unnest(ARRAY['pg_tviews.graph_cache_enabled', 'pg_tviews.table_cache_enabled',
                    'pg_tviews.direct_patch_enabled', 'pg_tviews.max_queue_size',
                    'pg_tviews.publish_changes',
                    'pg_tviews.max_refresh_rows', 'pg_tviews.audit_enabled',
                    'pg_tviews.lock_escalation_threshold']) n;
RESET ROLE;
DROP ROLE regress_gucs_user;

-- 3. lock_escalation_threshold: 64 by default, 0 (always escalate) at least;
--    "never escalate" (-1) is gone: it could exhaust the shared lock table.
SELECT must(current_setting('pg_tviews.lock_escalation_threshold') = '64', 'threshold default');
SELECT must(outcome($$SET pg_tviews.lock_escalation_threshold = 0$$) = 'ok', 'threshold 0');
SELECT must(outcome($$SET pg_tviews.lock_escalation_threshold = -1$$) = '22023',
            'threshold -1 accepted');
RESET pg_tviews.lock_escalation_threshold;

-- 4. The settings that became constants are refused by name.
SELECT must(outcome(format('SET %s = 1', n)) = '42602', n || ' still accepted')
  FROM unnest(ARRAY['pg_tviews.max_propagation_depth', 'pg_tviews.max_dependency_depth',
                    'pg_tviews.batch_size',
                    'pg_tviews.cache_size', 'pg_tviews.report_max_tracked']) n;

-- 5. A bulk change past one batch (1,000 keys) is refreshed whole, in batches.
CREATE TABLE tb_tenant (
    pk_tenant INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name      TEXT
);
CREATE TABLE tb_order (
    pk_order   INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    identifier TEXT NOT NULL,
    fk_tenant  INTEGER NOT NULL REFERENCES tb_tenant(pk_tenant),
    status     TEXT DEFAULT 'draft'
);
INSERT INTO tb_tenant (name) VALUES ('T1');

SELECT pg_tviews_create('tv_order', $TVIEW$
    SELECT o.pk_order AS pk_order, o.id AS id, o.fk_tenant AS fk_tenant,
           jsonb_build_object('id', o.id::text, 'identifier', o.identifier,
                              'status', o.status, 'tenant_name', t.name) AS data
    FROM tb_order o JOIN tb_tenant t ON t.pk_tenant = o.fk_tenant
$TVIEW$);

INSERT INTO tb_order (identifier, fk_tenant, status)
  SELECT 'o' || g, 1, 'draft' FROM generate_series(1, 2500) g;
SELECT must((SELECT count(*) FROM tv_order) = 2500, 'a bulk insert lost rows');
UPDATE tb_tenant SET name = 'T2';
SELECT must((SELECT count(*) FROM tv_order WHERE data->>'tenant_name' = 'T2') = 2500,
            'a cascade past one batch left rows stale');
DELETE FROM tb_order WHERE pk_order % 2 = 1;
SELECT must((SELECT count(*) FROM tv_order) = 1250, 'a bulk delete left rows');

\echo 'gucs: PASS'
