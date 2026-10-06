-- Regression test (#151): tviews.registry reports the backing view.
--
-- Tools need each TVIEW's backing view v_<entity>, which was only in the internal
-- pg_tview_meta. The registry's `view` column is a regclass, appended last (an
-- addition: contract_version() stays 1), NULL when the view is gone.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_151_registry_view.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE SCHEMA app;
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE app.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice');
INSERT INTO app.tb_post (pk_post, title) VALUES (1, 'p1');

SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SET search_path TO app, public, tviews;
SELECT pg_tviews_create('tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM app.tb_post $$);
RESET search_path;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#151 FAIL: %', what; END IF; END $$;

-- 1. `view` is a regclass appended after the contract-1 columns (later additions
-- come after it); the contract version is unchanged.
SELECT must(
    (SELECT attname || ' ' || format_type(atttypid, atttypmod) FROM pg_attribute
     WHERE attrelid = 'tviews.registry'::regclass AND attnum = 9) = 'view regclass',
    'the ninth registry column is not view regclass');
SELECT must(tviews.contract_version() = 1, 'contract_version() changed');

-- 2. It names the backing view, in the extension's schema (#181).
SELECT must((SELECT view FROM tviews.registry WHERE entity = 'user') = 'tviews.public__tv_user'::regclass,
            'tv_user view');
SELECT must((SELECT view FROM tviews.registry WHERE entity = 'post') = 'tviews.app__tv_post'::regclass,
            'tv_post view');
SET search_path TO public;
SELECT must((SELECT view::text FROM tviews.registry WHERE entity = 'post') = 'tviews.app__tv_post',
            'tv_post view is not printed schema-qualified off the search_path');
RESET search_path;

-- 3. A registration whose view is gone reports NULL, not a dangling OID.
UPDATE tviews.pg_tview_meta SET view_oid = 4000000000::oid::regclass WHERE entity = 'user';
SELECT must((SELECT view IS NULL FROM tviews.registry WHERE entity = 'user'),
            'a missing view is not NULL: '
            || (SELECT view::text FROM tviews.registry WHERE entity = 'user'));
UPDATE tviews.pg_tview_meta SET view_oid = 'tviews.public__tv_user'::regclass WHERE entity = 'user';

DROP SCHEMA app CASCADE;
DROP EXTENSION pg_tviews CASCADE;

SELECT 'issue #151 registry view: PASS' AS result;
-- expect-output: issue #151 registry view: PASS
