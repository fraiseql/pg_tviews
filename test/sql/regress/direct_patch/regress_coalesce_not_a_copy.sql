-- Regression test: a column under COALESCE is no plain copy into data.
--
-- `jsonb_build_object('bio', COALESCE(u.bio, 'none'))` was read as a field
-- copying u.bio, so a direct patch (the root row) and a fan-out patch (the
-- embedding rows) wrote null where the view computes 'none'.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/direct_patch/regress_coalesce_not_a_copy.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), bio text, name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int REFERENCES tb_user, title text);
INSERT INTO tb_user VALUES (1, default, 'b', 'n');
INSERT INTO tb_post VALUES (1, default, 1, 't'), (2, default, 1, 't2');
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('bio', COALESCE(bio, 'none'), 'name', name) AS data FROM tb_user $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'bio', COALESCE(u.bio, 'none'), 'name', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);

CREATE FUNCTION assert_fresh(what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT pk_user, data FROM tv_user EXCEPT SELECT pk_user, data FROM tviews.public__tv_user)
       OR EXISTS (SELECT pk_post, data FROM tv_post EXCEPT SELECT pk_post, data FROM tviews.public__tv_post) THEN
        RAISE EXCEPTION 'FAIL after %: % / %', what,
            (SELECT jsonb_agg(data) FROM tv_user), (SELECT jsonb_agg(data) FROM tv_post);
    END IF;
END $$;

UPDATE tb_user SET bio = NULL;
SELECT assert_fresh('bio set to NULL');
UPDATE tb_user SET bio = 'back';
SELECT assert_fresh('bio set again');
-- A plain copy next to it is still patched.
UPDATE tb_user SET name = 'm';
SELECT assert_fresh('name');

SELECT 'coalesce not a copy: PASS' AS result;
-- expect-output: coalesce not a copy: PASS
