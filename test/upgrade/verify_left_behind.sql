-- Checks on the catalog of fixtures_left_behind.sql once ALTER EXTENSION
-- pg_tviews UPDATE completed (test/upgrade/upgrade_check.sh): the TVIEWs it
-- could not re-derive keep needs_reregister, the health check names them, their
-- refreshes skip and patch nothing and fail where their definition no longer
-- holds, and each is cleared by a fix and a re-registration.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET search_path TO pg_catalog;

CREATE FUNCTION pg_temp.must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'upgrade check: %', what; END IF; END $$;
CREATE FUNCTION pg_temp.in_sync(tview text, key text) RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE differ boolean;
BEGIN
    EXECUTE format('SELECT EXISTS (SELECT 1 FROM public.%1$I t FULL JOIN %2$s v USING (%3$I) '
                   'WHERE t.data IS DISTINCT FROM v.data)',
                   tview, (SELECT view FROM tviews.registry WHERE name = tview), key) INTO differ;
    RETURN NOT differ;
END $$;
CREATE FUNCTION pg_temp.reregister_health() RETURNS text LANGUAGE sql AS $$
    SELECT status || ' ' || message FROM tviews.pg_tviews_check_health()
    WHERE component = 'reregister' $$;
-- The work a statement did, from the session's counters (kept per transaction).
CREATE TEMP TABLE seen (q jsonb);
CREATE FUNCTION pg_temp.mark() RETURNS void LANGUAGE sql AS $$
    DELETE FROM seen; INSERT INTO seen SELECT tviews.pg_tviews_show_queue() $$;
CREATE FUNCTION pg_temp.work() RETURNS jsonb LANGUAGE sql AS $$
    SELECT jsonb_build_object(
        'recomputes', (n->>'view_recomputes')::int - (s.q->>'view_recomputes')::int,
        'captured', (n->>'direct_patch_captured')::int - (s.q->>'direct_patch_captured')::int)
    FROM seen s, tviews.pg_tviews_show_queue() AS now(n) $$;

-- Left exactly the four it could not re-derive.
SELECT pg_temp.must(string_agg(name, ',' ORDER BY name) = 'tv_col,tv_forced,tv_lost,tv_rls',
                    'left to re-register: ' || coalesce(string_agg(name, ',' ORDER BY name), 'none'))
FROM tviews.registry WHERE needs_reregister;

-- The health check names each, and why when it knows.
SELECT pg_temp.must(pg_temp.reregister_health() LIKE 'WARNING 4 TVIEWs left to re-register:%'
                    AND pg_temp.reregister_health() LIKE '%tv_col: upgrade_lost_owner lacks SELECT (title) on%'
                    AND pg_temp.reregister_health() LIKE '%tv_forced;%'
                    AND pg_temp.reregister_health() LIKE '%tv_lost: upgrade_lost_owner lacks SELECT on%'
                    AND pg_temp.reregister_health() LIKE '%tv_rls: row security applies to role upgrade_rls_owner%',
                    'health: ' || pg_temp.reregister_health());

-- tv_kept was re-derived and follows its table.
UPDATE public.tb_kept SET title = 'k1b';
SELECT pg_temp.must(pg_temp.in_sync('tv_kept', 'pk_kept'), 'tv_kept after a write');

-- tv_forced keeps the previous release's plan, which stores no read set: its
-- tables and paths read as unknown.
SELECT pg_temp.must(NOT EXISTS (
        SELECT 1 FROM tviews.pg_tview_meta m,
             pg_catalog.jsonb_array_elements(
                 (m.plan OPERATOR(pg_catalog.->) 'tables')
                 OPERATOR(pg_catalog.||) COALESCE(m.plan OPERATOR(pg_catalog.->) 'paths',
                                                  '[]'::pg_catalog.jsonb)) e
        WHERE m.entity = 'forced' AND e OPERATOR(pg_catalog.?) 'read_set'),
    'the left-behind tv_forced has a read set');
-- So it skips nothing and patches nothing: an UPDATE of a column nothing
-- reads recomputes its row, one of a copied column is not patched.
BEGIN;
SELECT pg_temp.mark();
UPDATE public.tb_forced SET note = 'n' WHERE pk_forced = 1;
SELECT pg_temp.must(pg_temp.work()->>'recomputes' = '1',
                    'an UPDATE of a column tv_forced does not read was skipped: ' || pg_temp.work()::text);
COMMIT;
BEGIN;
SELECT pg_temp.mark();
UPDATE public.tb_forced SET name = 'f1b' WHERE pk_forced = 1;
SELECT pg_temp.must(pg_temp.work() = '{"recomputes": 1, "captured": 0}',
                    'an UPDATE of a column tv_forced copies was patched: ' || pg_temp.work()::text);
COMMIT;
SELECT pg_temp.must(pg_temp.in_sync('tv_forced', 'pk_forced'), 'tv_forced after the writes');

-- tv_lost's refresh reads as its owner and fails; tv_rls is refused. The
-- refresh runs at the end of the writing statement: read its SQLSTATE in psql.
\set ON_ERROR_STOP off
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_lost SET title = 'l1b';
SELECT :'LAST_ERROR_SQLSTATE' AS lost_state \gset
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_rls SET title = 'r1b';
SELECT :'LAST_ERROR_SQLSTATE' AS rls_state \gset
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_col SET title = 'c1b';
SELECT :'LAST_ERROR_SQLSTATE' AS col_state \gset
\set ON_ERROR_STOP on
SELECT pg_temp.must(:'lost_state' = '42501', 'a write to tb_lost: ' || :'lost_state');
SELECT pg_temp.must(:'rls_state' = '42P16', 'a write to tb_rls: ' || :'rls_state');
SELECT pg_temp.must(:'col_state' = '42501', 'a write to tb_col: ' || :'col_state');
SELECT pg_temp.must(data->>'title' = 'c1', 'tv_col changed') FROM public.tv_col;
SELECT pg_temp.must(data->>'title' = 'l1', 'tv_lost changed') FROM public.tv_lost;
SELECT pg_temp.must(data->>'title' = 'r1', 'tv_rls changed') FROM public.tv_rls;

-- Fixed and re-registered, each is cleared.
UPDATE tviews.pg_tview_meta SET definition = left(definition, -length('; SELECT 1'))
 WHERE entity = 'forced';
GRANT SELECT ON public.tb_lost TO upgrade_lost_owner;
ALTER TABLE public.tb_rls DISABLE ROW LEVEL SECURITY;
GRANT SELECT ON public.tb_col TO upgrade_lost_owner;
SELECT pg_temp.must(status = 'reregistered', 'tv_' || entity || ': ' || status)
FROM tviews.pg_tviews_reregister_all();
SELECT pg_temp.must(pg_temp.reregister_health() LIKE 'OK %', 'health after the fixes: ' || pg_temp.reregister_health());
UPDATE public.tb_lost SET title = 'l1c';
UPDATE public.tb_rls SET title = 'r1c';
SELECT pg_temp.must(pg_temp.in_sync('tv_lost', 'pk_lost'), 'tv_lost after the fix');
SELECT pg_temp.must(pg_temp.in_sync('tv_rls', 'pk_rls'), 'tv_rls after the fix');
UPDATE public.tb_col SET title = 'c1c';
SELECT pg_temp.must(pg_temp.in_sync('tv_col', 'pk_col'), 'tv_col after the fix');
