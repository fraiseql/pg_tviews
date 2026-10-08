-- Regression test: a backing view's privileges follow its TVIEW's table.
--
-- A backing view lives in tviews (#181). A role that read it through
-- GRANT SELECT ON ALL TABLES IN SCHEMA <app> when it was <app>.v_<entity> lost it:
-- fraisier's deploy probe (SELECT EXISTS (… FROM <backing view>), the view taken
-- from tviews.registry) was denied. Whoever can SELECT from tv_<entity> can now
-- SELECT from its backing view: the SELECT grants are copied when the view is
-- created or rebuilt, and again after every GRANT or REVOKE on a table; ALTER
-- TABLE tv_* OWNER TO gives the view the new owner. Only SELECT is copied.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_backing_view_privileges.sql
--
-- expect-output: backing view privileges: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'regress_bvp_owner') THEN
        DROP OWNED BY regress_bvp_owner, regress_bvp_owner2, regress_bvp_reader, regress_bvp_other;
    END IF;
END $$;
DROP ROLE IF EXISTS regress_bvp_reader;
DROP ROLE IF EXISTS regress_bvp_other;
DROP ROLE IF EXISTS regress_bvp_owner2;
DROP ROLE IF EXISTS regress_bvp_owner;
CREATE ROLE regress_bvp_owner;
CREATE ROLE regress_bvp_owner2;
CREATE ROLE regress_bvp_reader;
CREATE ROLE regress_bvp_other;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'backing view privileges FAIL: %', what; END IF; END $$;

-- fraisier's find_empty_tviews, as role r: 'ok', or 'denied' with what was refused.
CREATE FUNCTION probe(r text, tv text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE
    tview text;
    view text;
    has_rows boolean;
    view_has_rows boolean;
BEGIN
    SELECT format('%I.%I', g.schema, g.name), g.view::text INTO tview, view
    FROM tviews.registry g WHERE g.name = tv;
    EXECUTE format('SET LOCAL ROLE %I', r);
    EXECUTE format('SELECT EXISTS (SELECT 1 FROM %s), EXISTS (SELECT 1 FROM %s)', tview, view)
        INTO has_rows, view_has_rows;
    RESET ROLE;
    RETURN 'ok';
EXCEPTION WHEN insufficient_privilege THEN
    RETURN 'denied';
END $$;

CREATE SCHEMA app AUTHORIZATION regress_bvp_owner;
SET ROLE regress_bvp_owner;
CREATE TABLE app.tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO app.tb_post (pk_post, title) VALUES (1, 'a'), (2, 'b');
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM app.tb_post $$);

-- SELECT on the base table only: neither the TVIEW nor its view is readable.
GRANT USAGE ON SCHEMA app TO regress_bvp_reader;
GRANT SELECT ON app.tb_post TO regress_bvp_reader;
SELECT must(probe('regress_bvp_reader', 'tv_post') = 'denied', 'SELECT on tb_post alone reads the TVIEW');

-- SELECT ON ALL TABLES IN SCHEMA: the backing view is readable too (fraisier's case).
GRANT SELECT ON ALL TABLES IN SCHEMA app TO regress_bvp_reader;
SELECT must(probe('regress_bvp_reader', 'tv_post') = 'ok', 'denied after GRANT ON ALL TABLES IN SCHEMA');

-- A REVOKE takes it away from both.
REVOKE SELECT ON ALL TABLES IN SCHEMA app FROM regress_bvp_reader;
SELECT must(probe('regress_bvp_reader', 'tv_post') = 'denied', 'readable after REVOKE ON ALL TABLES');

-- A GRANT on the TVIEW's table alone.
GRANT SELECT ON app.tv_post TO regress_bvp_reader;
SELECT must(probe('regress_bvp_reader', 'tv_post') = 'ok', 'denied after GRANT SELECT ON tv_post');

-- A rebuild, then a replacement in place, keep it.
SELECT must(tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, title, jsonb_build_object('title', title) AS data FROM app.tb_post $$) = 'rebuilt',
            'the rebuild');
SELECT must(probe('regress_bvp_reader', 'tv_post') = 'ok', 'denied after a rebuild');
SELECT must(tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, title, jsonb_build_object('title', upper(title)) AS data FROM app.tb_post $$) = 'replaced',
            'the replacement in place');
SELECT must(probe('regress_bvp_reader', 'tv_post') = 'ok', 'denied after a replacement in place');

-- Only SELECT: what else the table grants is not copied to the view.
GRANT INSERT, UPDATE, DELETE, TRUNCATE ON app.tv_post TO regress_bvp_reader;
SELECT must(NOT has_table_privilege('regress_bvp_reader', g.view, 'INSERT, UPDATE, DELETE, TRUNCATE'),
            'a privilege other than SELECT reached the backing view')
FROM tviews.registry g WHERE g.name = 'tv_post';
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON app.tv_post FROM regress_bvp_reader;

-- PUBLIC, granted and revoked.
SELECT must(probe('regress_bvp_other', 'tv_post') = 'denied', 'regress_bvp_other reads before any grant');
GRANT USAGE ON SCHEMA app TO PUBLIC;
GRANT SELECT ON app.tv_post TO PUBLIC;
SELECT must(probe('regress_bvp_other', 'tv_post') = 'ok', 'denied after GRANT TO PUBLIC');
REVOKE SELECT ON app.tv_post FROM PUBLIC;
SELECT must(probe('regress_bvp_other', 'tv_post') = 'denied', 'readable after REVOKE FROM PUBLIC');

-- A TVIEW created under default privileges gets the view readable at once.
ALTER DEFAULT PRIVILEGES IN SCHEMA app GRANT SELECT ON TABLES TO regress_bvp_other;
CREATE TABLE app.tb_note (pk_note bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), body text);
INSERT INTO app.tb_note (pk_note, body) VALUES (1, 'n');
SELECT tviews.pg_tviews_create_or_replace('app.tv_note', $$
    SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM app.tb_note $$);
SELECT must(probe('regress_bvp_other', 'tv_note') = 'ok', 'denied under default privileges');
ALTER DEFAULT PRIVILEGES IN SCHEMA app REVOKE SELECT ON TABLES FROM regress_bvp_other;
RESET ROLE;

-- ALTER TABLE … OWNER TO: the view changes owner with its table, keeps the grants,
-- and the TVIEW keeps following its base table.
GRANT CREATE, USAGE ON SCHEMA app TO regress_bvp_owner2;
GRANT SELECT ON app.tb_post TO regress_bvp_owner2;
ALTER TABLE app.tv_post OWNER TO regress_bvp_owner2;
SELECT must(v.relowner = 'regress_bvp_owner2'::regrole, 'the view is owned by ' || v.relowner::regrole::text)
FROM tviews.registry g JOIN pg_class v ON v.oid = g.view WHERE g.name = 'tv_post';
SELECT must(probe('regress_bvp_reader', 'tv_post') = 'ok', 'denied after OWNER TO');
UPDATE app.tb_post SET title = 'a2' WHERE pk_post = 1;
SELECT assert_fresh('app.tv_post', 'pk_post', 'a write after OWNER TO');

-- The same by a non-superuser owner, giving the TVIEW to a role it belongs to.
GRANT regress_bvp_owner TO regress_bvp_owner2;
SET ROLE regress_bvp_owner2;
ALTER TABLE app.tv_post OWNER TO regress_bvp_owner;
RESET ROLE;
SELECT must(v.relowner = 'regress_bvp_owner'::regrole,
            'given back by its owner, the view is owned by ' || v.relowner::regrole::text)
FROM tviews.registry g JOIN pg_class v ON v.oid = g.view WHERE g.name = 'tv_post';
SELECT must(NOT has_schema_privilege('regress_bvp_owner', 'tviews', 'CREATE'),
            'the new owner kept CREATE on tviews');
SELECT must(probe('regress_bvp_reader', 'tv_post') = 'ok', 'denied after OWNER TO by the owner');

-- A grant on the view itself does not outlive the next sync.
GRANT SELECT ON ALL TABLES IN SCHEMA tviews TO regress_bvp_other;
SELECT must(NOT has_table_privilege('regress_bvp_other', g.view, 'SELECT'),
            'a grant on the backing view alone survived the sync')
FROM tviews.registry g WHERE g.name = 'tv_post';

DROP SCHEMA app CASCADE;
REVOKE ALL ON ALL TABLES IN SCHEMA tviews FROM regress_bvp_other;
DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY regress_bvp_owner, regress_bvp_owner2, regress_bvp_reader, regress_bvp_other;
DROP ROLE regress_bvp_reader;
DROP ROLE regress_bvp_other;
DROP ROLE regress_bvp_owner2;
DROP ROLE regress_bvp_owner;

\echo 'backing view privileges: PASS'
