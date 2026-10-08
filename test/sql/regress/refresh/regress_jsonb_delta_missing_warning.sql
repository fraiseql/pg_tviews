-- Regression test for issue #159:
--   "jsonb_delta not installed WARNING on every cascading write"
--
-- Without the optional jsonb_delta extension, every refresh that would have used
-- smart patching sent `WARNING: jsonb_delta extension not installed. Smart
-- patching disabled. ...` to the client: an application saw it on every write.
--
-- Correct behaviour: writes send nothing about jsonb_delta to the client. Each
-- backend logs the degraded mode once, at LOG (server log only). CREATE EXTENSION
-- pg_tviews says it once (a WARNING: an install script's NOTICEs are hidden), and
-- the health check reports it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_jsonb_delta_missing_warning.sql
--
-- expect-once: WARNING:  smart JSONB patching is disabled: jsonb_delta is not installed
-- expect-once: LOG:  pg_tviews: jsonb_delta is not installed
-- expect-output: issue #159 jsonb_delta warning: PASS
-- reject-output: WARNING:  jsonb_delta
-- reject-output: NOTICE:  jsonb_delta

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
SET client_min_messages TO NOTICE;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text);
CREATE TABLE tb_post (
    pk_post bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user bigint REFERENCES tb_user,
    title   text);
INSERT INTO tb_user (name) SELECT 'u' || g FROM generate_series(1, 3) g;
INSERT INTO tb_post (fk_user, title) SELECT 1 + g % 3, 'p' || g FROM generate_series(1, 9) g;

SET client_min_messages TO WARNING;
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
-- A base-table join: a scalar dependency, refreshed with smart patching when
-- jsonb_delta is there.
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
SET pg_tviews.direct_patch_enabled = off;

-- Ten cascading writes: nothing about jsonb_delta reaches the client.
SET client_min_messages TO NOTICE;
UPDATE tb_user SET name = name || '1' WHERE pk_user = 1;
UPDATE tb_user SET name = name || '2' WHERE pk_user = 2;
UPDATE tb_post SET title = title || '!' WHERE pk_post = 1;
UPDATE tb_post SET title = title || '!' WHERE pk_post = 2;
UPDATE tb_user SET name = name || '3' WHERE pk_user = 3;
UPDATE tb_post SET title = title || '?' WHERE pk_post = 3;
UPDATE tb_user SET name = name || '4' WHERE pk_user = 1;
UPDATE tb_post SET title = title || '?' WHERE pk_post = 4;
UPDATE tb_user SET name = name || '5' WHERE pk_user = 2;
UPDATE tb_post SET title = title || '#' WHERE pk_post = 5;

-- A new backend logs it once, however many writes it refreshes (LOG goes to the
-- client too when client_min_messages asks for it).
\c
SET pg_tviews.direct_patch_enabled = off;
SET client_min_messages TO LOG;
UPDATE tb_user SET name = name || 'a' WHERE pk_user = 1;
UPDATE tb_post SET title = title || 'a' WHERE pk_post = 1;
UPDATE tb_user SET name = name || 'b' WHERE pk_user = 2;
UPDATE tb_post SET title = title || 'b' WHERE pk_post = 2;
SET client_min_messages TO WARNING;

DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_post t JOIN tviews.public__tv_post v USING (pk_post) WHERE t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'FAIL #159: tv_post diverges from tviews.public__tv_post';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check() h
                   WHERE h::text LIKE '%jsonb_delta not installed%') THEN
        RAISE EXCEPTION 'FAIL #159: the health check does not report the missing jsonb_delta';
    END IF;
END $$;

\echo 'issue #159 jsonb_delta warning: PASS'
