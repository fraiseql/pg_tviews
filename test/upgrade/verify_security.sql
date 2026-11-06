-- Checks on the fixtures of fixtures_security.sql once ALTER EXTENSION pg_tviews
-- UPDATE ran (test/upgrade/upgrade_check.sh).

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET search_path TO pg_catalog;

CREATE FUNCTION pg_temp.must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'upgrade check: %', what; END IF; END $$;
CREATE FUNCTION pg_temp.in_sync(tview text, key text) RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE differ boolean;
BEGIN
    EXECUTE format('SELECT EXISTS (SELECT 1 FROM sec.%1$I t FULL JOIN %2$s v USING (%3$I) '
                   'WHERE t.data IS DISTINCT FROM v.data)',
                   tview, (SELECT view FROM tviews.registry WHERE name = tview), key) INTO differ;
    RETURN NOT differ;
END $$;

-- The update re-derived tv_doc as its owner: planning its mapping queries ran
-- the owner's code as the owner, never as the role running the update.
SELECT pg_temp.must(count(*) > 0, 'the update did not plan tv_doc''s mapping queries')
FROM sec.who_log;
SELECT pg_temp.must(count(*) = 0, 'the update ran a TVIEW owner''s code as ' || string_agg(DISTINCT cu, ', '))
FROM sec.who_log WHERE cu <> 'upgrade_owner';

-- tv_doc's backing view is its owner's alone, column grants included.
SELECT pg_temp.must(NOT has_column_privilege('upgrade_column_reader', view, 'data', 'SELECT'),
                    'a column grant on a backing view survived the update')
FROM tviews.registry WHERE name = 'tv_doc';

-- tv_tag calls a function whose search_path does not end in pg_temp: reported.
SELECT pg_temp.must(status = 'ERROR' AND message LIKE '%sec.label(text)%' AND message LIKE '%tv_tag%',
                    format('function_paths: %s %s', status, message))
FROM tviews.pg_tviews_check_health() WHERE component = 'function_paths';

-- tv_shop's guards follow the current rules: the char(2) constant keeps its
-- length, and IS NULL over the outer join's nullable side is no guard.
SELECT pg_temp.must(t->>'guard' !~ '::character([),\]]|\[)' AND t->>'guard' NOT LIKE '%IS NULL%',
                    'a guard re-derived by the update breaks the current rules: ' || (t->>'guard'))
FROM tviews.pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t
WHERE m.entity = 'shop' AND t ? 'guard';
UPDATE sec.tb_country SET name = 'République française' WHERE code = 'FR';
SELECT pg_temp.must(pg_temp.in_sync('tv_shop', 'pk_shop'), 'tv_shop after a write meeting its guard');
INSERT INTO sec.tb_notice VALUES (1, 'closed');
SELECT pg_temp.must(pg_temp.in_sync('tv_shop', 'pk_shop'), 'tv_shop after a row its anti-join reads');
DELETE FROM sec.tb_notice;
SELECT pg_temp.must(pg_temp.in_sync('tv_shop', 'pk_shop'), 'tv_shop after that row went');
