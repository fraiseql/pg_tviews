-- Regression test for the #44 suspend / resume API.
--
-- pg_tviews_suspend_triggers() set a flag the row trigger never read, so every write
-- still refreshed row by row; and pg_tviews_resume_triggers() enqueued pk 0 for each
-- changed entity, which refreshes nothing. Now suspension skips refresh, and resuming
-- (or committing while still suspended) rebuilds every TVIEW the suspended writes
-- touched, including TVIEWs that embed them.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_44_suspend_resume.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    title   TEXT
);
INSERT INTO tb_user (name) SELECT 'u' || g FROM generate_series(1, 50) g;
INSERT INTO tb_post (fk_user, title) SELECT 1 + g % 50, 't' || g FROM generate_series(1, 500) g;
CREATE TABLE tv_user AS SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user;
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', tv_user.data) AS data
FROM tb_post p JOIN tv_user ON tv_user.pk_user = p.fk_user;

CREATE FUNCTION must(ok BOOLEAN, msg TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION '#44 FAIL: %', msg; END IF; END $$;
CREATE FUNCTION in_sync() RETURNS BOOLEAN LANGUAGE sql AS $$
    SELECT (SELECT count(*) FROM tviews.public__tv_user v FULL JOIN tv_user t USING (pk_user)
            WHERE t.data IS DISTINCT FROM v.data) = 0
       AND (SELECT count(*) FROM tviews.public__tv_post v FULL JOIN tv_post t USING (pk_post)
            WHERE t.data IS DISTINCT FROM v.data) = 0
$$;

-- ========================================================================
-- Cycle 1: suspended writes are not refreshed; resuming rebuilds, dependents too
-- ========================================================================
BEGIN;
SELECT pg_tviews_suspend_triggers();
UPDATE tb_user SET name = name || '!';
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 1) = 'u1',
            'tv_user was refreshed while suspended');
SELECT pg_tviews_resume_triggers();
SELECT must(in_sync(), 'tv_user / tv_post (which embeds tviews.public__tv_user) not rebuilt on resume');
COMMIT;

-- ========================================================================
-- Cycle 2: nested suspension only resumes at the outermost resume
-- ========================================================================
BEGIN;
SELECT pg_tviews_suspend_triggers();
SELECT pg_tviews_suspend_triggers();
UPDATE tb_post SET title = title || '?' WHERE pk_post <= 10;
SELECT pg_tviews_resume_triggers();
SELECT must(pg_tviews_is_suspended(), 'inner resume ended the suspension');
SELECT must((SELECT data->>'title' FROM tv_post WHERE pk_post = 1) NOT LIKE '%?',
            'refresh ran before the outermost resume');
SELECT pg_tviews_resume_triggers();
SELECT must(NOT pg_tviews_is_suspended() AND in_sync(), 'outermost resume');
COMMIT;

-- ========================================================================
-- Cycle 3: committing while still suspended catches up before COMMIT
-- ========================================================================
BEGIN;
SELECT pg_tviews_suspend_triggers();
DELETE FROM tb_post WHERE pk_post > 490;
INSERT INTO tb_post (fk_user, title) VALUES (2, 'new');
COMMIT;
SELECT must(NOT pg_tviews_is_suspended(), 'still suspended after COMMIT');
SELECT must(in_sync(), 'COMMIT without resume left TVIEWs stale');

-- Incremental refresh works again afterwards.
UPDATE tb_user SET name = 'z' WHERE pk_user = 3;
SELECT must(in_sync(), 'incremental refresh after the suspension');
