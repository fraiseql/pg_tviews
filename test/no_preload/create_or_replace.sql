-- pg_tviews_create_or_replace() in a cluster that does not preload pg_tviews (#134).
--
-- The function does all its work itself: it needs neither the ProcessUtility hook
-- nor the event trigger, so it works in a session that loaded the library lazily,
-- in the same batch as CREATE EXTENSION. The TVIEWs it creates then follow their
-- base tables in later sessions, where the triggers load the library.
--
--   test/no_preload/run.sh   (starts a throwaway cluster without the preload)

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DO $$ BEGIN
    IF current_setting('shared_preload_libraries') LIKE '%pg_tviews%' THEN
        RAISE EXCEPTION 'no-preload FAIL: this cluster preloads pg_tviews';
    END IF;
END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');

-- Same batch as CREATE EXTENSION, in a session that has not loaded the library.
CREATE EXTENSION jsonb_delta \; CREATE EXTENSION pg_tviews \; SELECT tviews.pg_tviews_create_or_replace('tv_user', $$ SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$) AS created \gset
\if :{?created}
\else
    \echo 'no-preload FAIL: create_or_replace returned nothing'
    \quit
\endif

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'no-preload FAIL: %', what; END IF; END $$;
SELECT must(:'created' = 'created', 'created in the CREATE EXTENSION batch');
SELECT must((SELECT count(*) FROM tv_user) = 2, 'populated');

-- A new session: the library is loaded by the base-table trigger.
\c
SET client_min_messages TO WARNING;
UPDATE tb_user SET name = 'alicia' WHERE pk_user = 1;
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 1) = 'alicia',
            'refreshed at the end of the statement');
BEGIN;
INSERT INTO tb_user (pk_user, name) VALUES (3, 'carol');
COMMIT;
SELECT must(EXISTS (SELECT 1 FROM tv_user WHERE pk_user = 3), 'refreshed in a transaction');

-- Another new session: a column change rebuilds, in a DO block.
\c
SET client_min_messages TO WARNING;
DO $$ BEGIN
    PERFORM must(tviews.pg_tviews_create_or_replace('tv_user', $x$
        SELECT pk_user, id, name, jsonb_build_object('name', upper(name)) AS data FROM tb_user $x$)
        = 'rebuilt', 'rebuilt in a DO block');
END $$;
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 2) = 'BOB', 'rows recomputed');
SELECT must(tviews.pg_tviews_create_or_replace('tv_user',
    (SELECT query FROM tviews.registry WHERE entity = 'user')) = 'unchanged', 'round trip');

-- Statistics live in shared memory the library asks for when preloaded: without
-- it, tviews.stats says so instead of showing zeros.
DO $$
BEGIN
    PERFORM count(*) FROM tviews.stats;
    PERFORM must(false, 'tviews.stats read without the library preloaded');
EXCEPTION WHEN object_not_in_prerequisite_state THEN
    PERFORM must(SQLERRM LIKE '%shared_preload_libraries%', 'stats error: ' || SQLERRM);
END $$;

SELECT 'no-preload create_or_replace: PASS' AS result;
