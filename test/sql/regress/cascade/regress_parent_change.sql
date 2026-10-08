-- Moving a row to another parent must refresh both parents (prerequisite of #58).
--
-- The row trigger followed cascade paths from the NEW image only, so an UPDATE that
-- changed a child's FK refreshed the new parent and left the old one still listing
-- the child. Aggregate TVIEWs (#58) hit the same case when a row moves between
-- groups.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_parent_change.sql
-- expect-output: parent_change: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title   TEXT
);
CREATE TABLE tb_comment (
    pk_comment BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_post    BIGINT NOT NULL REFERENCES tb_post(pk_post),
    body       TEXT
);
INSERT INTO tb_post (title) VALUES ('p1'), ('p2');
INSERT INTO tb_comment (fk_post, body) VALUES (1, 'c1'), (1, 'c2');

CREATE TABLE tv_post AS
SELECT p.pk_post, p.id,
       jsonb_build_object('title', p.title,
                          'comments', coalesce(jsonb_agg(c.body ORDER BY c.body)
                                               FILTER (WHERE c.pk_comment IS NOT NULL), '[]')) AS data
FROM tb_post p LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
GROUP BY p.pk_post, p.id, p.title;

UPDATE tb_comment SET fk_post = 2 WHERE pk_comment = 1;

DO $$ BEGIN
  IF (SELECT count(*) FROM tviews.public__tv_post v FULL JOIN tv_post t USING (pk_post)
      WHERE t.data IS DISTINCT FROM v.data) <> 0 THEN
    RAISE EXCEPTION 'FAIL: moving a comment left tv_post = %, tviews.public__tv_post = %',
      (SELECT jsonb_object_agg(pk_post, data->'comments') FROM tv_post),
      (SELECT jsonb_object_agg(pk_post, data->'comments') FROM tviews.public__tv_post);
  END IF;
END $$;

\echo 'parent_change: PASS'
