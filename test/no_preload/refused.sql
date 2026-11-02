-- pg_tviews requires shared_preload_libraries: there is no mode without its hooks,
-- its shared statistics and its rebuild worker. run.sh calls this file in phases:
--
--   install    (no preload)   CREATE EXTENSION fails with 55000 and the hint;
--   setup      (preloaded)    a TVIEW is created;
--   unloaded   (no preload)   a write to its base table fails with 55000, and so do
--                             the functions: nothing runs half-installed.
--
--   test/no_preload/run.sh   (restarts a throwaway cluster between phases)

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

CREATE OR REPLACE FUNCTION pg_temp.must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'no-preload FAIL: %', what; END IF; END $$;
-- The SQLSTATE and message `stmt` fails with, or 'ok'.
CREATE OR REPLACE FUNCTION pg_temp.outcome(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'ok';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;

\if :{?phase}
\else
    \echo 'no-preload FAIL: run through run.sh (phase not set)'
    \quit
\endif

SELECT :'phase' = 'install' AS install, :'phase' = 'setup' AS setup,
       :'phase' = 'unloaded' AS unloaded \gset

\if :install
    SELECT pg_temp.must(current_setting('shared_preload_libraries') NOT LIKE '%pg_tviews%',
                        'this cluster preloads pg_tviews');
    CREATE EXTENSION jsonb_delta;
    SELECT pg_temp.must(pg_temp.outcome('CREATE EXTENSION pg_tviews')
                        = '55000 pg_tviews is not in shared_preload_libraries',
                        'CREATE EXTENSION without the preload: '
                        || pg_temp.outcome('CREATE EXTENSION pg_tviews'));
    SELECT pg_temp.must(NOT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_tviews'),
                        'the extension was installed');
    SELECT 'no-preload install: PASS' AS result;
\endif

\if :setup
    CREATE EXTENSION pg_tviews;
    CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
    INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice');
    SELECT tviews.pg_tviews_create('tv_user',
        $$ SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
    SELECT 'no-preload setup: PASS' AS result;
\endif

\if :unloaded
    SELECT pg_temp.must(current_setting('shared_preload_libraries') NOT LIKE '%pg_tviews%',
                        'this cluster preloads pg_tviews');
    SELECT pg_temp.must(pg_temp.outcome(s) LIKE '55000 pg_tviews is not in shared_preload_libraries%',
                        s || ': ' || pg_temp.outcome(s))
      FROM unnest(ARRAY[
          $$UPDATE tb_user SET name = 'alicia' WHERE pk_user = 1$$,
          $$SELECT tviews.pg_tviews_refresh('tv_user')$$,
          $$SELECT tviews.pg_tviews_create_or_replace('tv_user',
              'SELECT pk_user, id, jsonb_build_object(''n'', name) AS data FROM tb_user')$$
      ]) s;
    SELECT pg_temp.must((SELECT name FROM tb_user) = 'alice', 'a refused write changed tb_user');
    SELECT 'no-preload unloaded: PASS' AS result;
\endif
