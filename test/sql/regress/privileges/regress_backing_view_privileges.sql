-- Regression test: a backing view is its TVIEW owner's alone.
--
-- A backing view (tviews.<schema>__tv_<entity>) runs the TVIEW's SELECT over the
-- base tables with its owner's privileges. A reader granted it would read every
-- row the definition yields, whatever policy restricts tv_<entity>: a reader that
-- row-level security denied every row of tv_post read them all through the view.
-- So no role but the owner holds a privilege on a backing view: no grant is
-- copied from the table, and one made on the view is revoked after every GRANT,
-- REVOKE, ALTER TABLE … OWNER TO and REASSIGN OWNED, and when the view is created
-- or rebuilt. ALTER TABLE tv_* OWNER TO gives the view the new owner. The deploy
-- probe that read backing views ("a TVIEW empty while its view has rows") is the
-- health check's `contents` component, which runs as each TVIEW's owner.
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

-- The rows role r reads from relation rel, or -1 when it is denied.
CREATE FUNCTION rows_as(r text, rel text) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE n bigint;
BEGIN
    EXECUTE format('SET LOCAL ROLE %I', r);
    EXECUTE format('SELECT count(*) FROM %s', rel) INTO n;
    RESET ROLE;
    RETURN n;
EXCEPTION WHEN insufficient_privilege THEN
    RETURN -1;
END $$;

-- Whether any role but the view's owner holds a privilege on tv's backing view.
CREATE FUNCTION view_granted(tv text) RETURNS boolean LANGUAGE sql AS $$
    SELECT EXISTS (SELECT 1 FROM tviews.registry g JOIN pg_class v ON v.oid = g.view,
                   aclexplode(v.relacl) a
                   WHERE g.name = tv AND a.grantee <> v.relowner) $$;

CREATE FUNCTION backing(tv text) RETURNS text LANGUAGE sql AS $$
    SELECT view::text FROM tviews.registry WHERE name = tv $$;

CREATE SCHEMA app AUTHORIZATION regress_bvp_owner;
SET ROLE regress_bvp_owner;
CREATE TABLE app.tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO app.tb_post (pk_post, title) VALUES (1, 'a'), (2, 'b');
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM app.tb_post $$);
GRANT USAGE ON SCHEMA app TO regress_bvp_reader;

-- Row-level security on the TVIEW's table holds: the backing view does not
-- hand the rows back.
GRANT SELECT ON app.tv_post TO regress_bvp_reader;
ALTER TABLE app.tv_post ENABLE ROW LEVEL SECURITY;
CREATE POLICY nothing ON app.tv_post FOR SELECT TO regress_bvp_reader USING (false);
SELECT must(rows_as('regress_bvp_reader', 'app.tv_post') = 0, 'the policy did not hide tv_post''s rows');
SELECT must(rows_as('regress_bvp_reader', backing('tv_post')) = -1,
            'the backing view handed back the rows the policy hides');
DROP POLICY nothing ON app.tv_post;
ALTER TABLE app.tv_post DISABLE ROW LEVEL SECURITY;

-- No grant on the table reaches the view: by name, in schema, to PUBLIC.
GRANT SELECT ON ALL TABLES IN SCHEMA app TO regress_bvp_reader;
GRANT INSERT, UPDATE, DELETE, TRUNCATE ON app.tv_post TO regress_bvp_reader;
GRANT USAGE ON SCHEMA app TO PUBLIC;
GRANT SELECT ON app.tv_post TO PUBLIC;
SELECT must(rows_as('regress_bvp_reader', 'app.tv_post') = 2, 'the reader cannot read tv_post');
SELECT must(NOT view_granted('tv_post'), 'a grant on the table reached the backing view');
SELECT must(rows_as('regress_bvp_other', backing('tv_post')) = -1, 'PUBLIC reads the backing view');

-- Nor does a rebuild, a replacement in place, or a TVIEW created under default
-- privileges.
SELECT must(tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, title, jsonb_build_object('title', title) AS data FROM app.tb_post $$) = 'rebuilt',
            'the rebuild');
SELECT must(NOT view_granted('tv_post'), 'a rebuild granted the backing view');
SELECT must(tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, title, jsonb_build_object('title', upper(title)) AS data FROM app.tb_post $$) = 'replaced',
            'the replacement in place');
SELECT must(NOT view_granted('tv_post'), 'a replacement in place granted the backing view');
ALTER DEFAULT PRIVILEGES IN SCHEMA app GRANT SELECT ON TABLES TO regress_bvp_other;
CREATE TABLE app.tb_note (pk_note bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), body text);
INSERT INTO app.tb_note (pk_note, body) VALUES (1, 'n');
SELECT tviews.pg_tviews_create_or_replace('app.tv_note', $$
    SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM app.tb_note $$);
SELECT must(rows_as('regress_bvp_other', 'app.tv_note') = 1, 'default privileges did not reach tv_note');
SELECT must(NOT view_granted('tv_note'), 'default privileges granted the backing view');
ALTER DEFAULT PRIVILEGES IN SCHEMA app REVOKE SELECT ON TABLES FROM regress_bvp_other;
REVOKE SELECT ON app.tv_post FROM PUBLIC;
RESET ROLE;

-- ALTER TABLE … OWNER TO: the view changes owner with its table, stays the
-- owner's, and the TVIEW keeps following its base table.
GRANT CREATE, USAGE ON SCHEMA app TO regress_bvp_owner2;
GRANT SELECT ON app.tb_post TO regress_bvp_owner2;
ALTER TABLE app.tv_post OWNER TO regress_bvp_owner2;
SELECT must(v.relowner = 'regress_bvp_owner2'::regrole, 'the view is owned by ' || v.relowner::regrole::text)
FROM tviews.registry g JOIN pg_class v ON v.oid = g.view WHERE g.name = 'tv_post';
SELECT must(NOT view_granted('tv_post'), 'OWNER TO granted the backing view');
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

-- A grant made on the view itself does not outlive the next sync.
GRANT SELECT ON ALL TABLES IN SCHEMA tviews TO regress_bvp_other;
SELECT must(NOT view_granted('tv_post'), 'a grant on the backing view survived the sync');

-- Nor does a grant on its columns, alone or with one on the view.
SELECT format('GRANT SELECT (pk_post, data) ON %s TO regress_bvp_reader', backing('tv_post')) \gexec
SELECT must(NOT has_column_privilege('regress_bvp_reader', backing('tv_post'), 'data', 'SELECT'),
            'a column grant on the backing view survived the sync');
SELECT must(rows_as('regress_bvp_reader', backing('tv_post')) = -1,
            'a column grant let the reader read the backing view');
SELECT format('GRANT SELECT ON %1$s TO regress_bvp_other; GRANT SELECT (data) ON %1$s TO regress_bvp_reader',
              backing('tv_post')) \gexec
SELECT must(NOT has_column_privilege(r, backing('tv_post'), 'data', 'SELECT'),
            format('a grant to %s on the backing view or its columns survived the sync', r))
FROM unnest(ARRAY['regress_bvp_reader', 'regress_bvp_other']) r;

-- The probe a reader can no longer run: health's `contents` component names a
-- TVIEW whose table is empty while its definition yields rows.
SELECT must(status = 'OK', 'contents on a populated TVIEW: ' || message)
FROM tviews.pg_tviews_check_health() WHERE component = 'contents';
SET session_replication_role = replica;
DELETE FROM app.tv_note;
RESET session_replication_role;
SELECT must(status = 'WARNING' AND message LIKE '%app.tv_note%' AND message NOT LIKE '%tv_post%',
            'contents on an emptied TVIEW: ' || message)
FROM tviews.pg_tviews_check_health() WHERE component = 'contents';
SELECT must(count(*) = 1, 'one contents row') FROM tviews.pg_tviews_check_health() WHERE component = 'contents';
-- What a definition yields is its owner's to know: a role that is not a member of
-- the owner is not told about tv_note.
SET ROLE regress_bvp_reader;
SELECT must(status = 'OK', 'a non-owner learned of an emptied TVIEW: ' || message)
FROM tviews.pg_tviews_check_health() WHERE component = 'contents';
RESET ROLE;

DROP SCHEMA app CASCADE;
DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY regress_bvp_owner, regress_bvp_owner2, regress_bvp_reader, regress_bvp_other;
DROP ROLE regress_bvp_reader;
DROP ROLE regress_bvp_other;
DROP ROLE regress_bvp_owner2;
DROP ROLE regress_bvp_owner;

\echo 'backing view privileges: PASS'
