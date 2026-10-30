-- Regression test for issue #263: the plan records, per table, the columns a
-- TVIEW's rows are linked by (joins, filters, groupings, ...). A transaction
-- whose writes change none of them, and insert or delete no row, takes no
-- shared value lock when it refreshes: two such writers can no longer wait for
-- each other in a cycle. A column only shown in a row (copied into `data`,
-- aggregated, ordered inside an aggregate) is no link; a read the analysis
-- cannot follow leaves the links unknown, and every write then counts.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_issue_263_link_columns.sql
--
-- expect-output: issue #263 link columns: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, bio text, active boolean NOT NULL DEFAULT true);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author bigint NOT NULL REFERENCES tb_user, title text,
                      published boolean NOT NULL DEFAULT true);
CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post bigint NOT NULL REFERENCES tb_post,
                         fk_author bigint NOT NULL REFERENCES tb_user, content text,
                         created_at timestamptz NOT NULL DEFAULT now());
INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'ada', ''), (2, 'bob', '');
INSERT INTO tb_post (pk_post, fk_author, title) VALUES (1, 1, 'pa'), (2, 2, 'pb');
INSERT INTO tb_comment (pk_comment, fk_post, fk_author, content)
VALUES (1, 2, 1, 'ada on pb'), (2, 1, 2, 'bob on pa');

-- The link columns of `tbl` in `entity`'s plan, by name in attnum order;
-- '<unknown>' when the plan says nothing.
CREATE FUNCTION links_of(entity text, tbl regclass) RETURNS text LANGUAGE sql AS $$
  SELECT CASE WHEN t ? 'links' THEN
           COALESCE((SELECT string_agg(a.attname, ',' ORDER BY a.attnum)
                     FROM jsonb_array_elements_text(t->'links') l(n)
                     JOIN pg_attribute a ON a.attrelid = tbl AND a.attnum = l.n::int), '')
         ELSE '<unknown>' END
  FROM tviews.pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t
  WHERE m.entity = $1 AND (t->>'relid')::oid = tbl $$;

CREATE FUNCTION expect(entity text, tbl regclass, want text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE got text := links_of(entity, tbl);
BEGIN
    IF got IS DISTINCT FROM want THEN
        RAISE EXCEPTION '#263 FAIL: links of % in tv_%: %, expected %', tbl, entity, got, want;
    END IF;
END $$;

-- ── #263's shape: grouped by the identity, comments aggregated ──────────────
SELECT tviews.pg_tviews_create('tv_post', $$
  SELECT p.pk_post, p.id,
         jsonb_build_object('title', p.title,
           'author', jsonb_build_object('name', u.name, 'bio', u.bio),
           'comments', COALESCE(jsonb_agg(jsonb_build_object('content', c.content,
               'author', jsonb_build_object('name', cu.name, 'bio', cu.bio))
             ORDER BY c.pk_comment) FILTER (WHERE c.pk_comment IS NOT NULL), '[]')) AS data
  FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author
  LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
  LEFT JOIN tb_user cu ON cu.pk_user = c.fk_author
  GROUP BY p.pk_post, p.id, p.title, u.name, u.bio $$);
-- name and bio are grouped by, but next to the identity: they only name the row.
SELECT expect('post', 'tb_user', 'pk_user');
SELECT expect('post', 'tb_post', 'pk_post,fk_author');
-- pk_comment is read by the FILTER; content and the ORDER BY inside jsonb_agg
-- only shape the value.
SELECT expect('post', 'tb_comment', 'pk_comment,fk_post,fk_author');

-- ── filters ─────────────────────────────────────────────────────────────────
SELECT tviews.pg_tviews_create('tv_feed', $$
  SELECT p.pk_post AS pk_feed, p.id, jsonb_build_object('title', p.title, 'by', u.name) AS data
  FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author
  WHERE p.published AND u.active $$);
SELECT expect('feed', 'tb_post', 'pk_post,fk_author,published');
SELECT expect('feed', 'tb_user', 'pk_user,active');

-- ── a correlated subquery under LIMIT: its ORDER BY picks the rows ───────────
SELECT tviews.pg_tviews_create('tv_latest', $$
  SELECT p.pk_post AS pk_latest, p.id,
         jsonb_build_object('title', p.title, 'latest', (
           SELECT jsonb_agg(x.content) FROM (
             SELECT c.content FROM tb_comment c WHERE c.fk_post = p.pk_post
             ORDER BY c.created_at DESC, c.pk_comment LIMIT 3) x)) AS data
  FROM tb_post p $$);
SELECT expect('latest', 'tb_comment', 'pk_comment,fk_post,created_at');
SELECT expect('latest', 'tb_post', 'pk_post');

-- ── a UNION removes duplicates by every column: links unknown ───────────────
SELECT tviews.pg_tviews_create('tv_person', $$
  SELECT u.pk_user AS pk_person, u.id, jsonb_build_object('name', u.name) AS data FROM tb_user u
  UNION
  SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) FROM tb_user u WHERE u.active $$);
SELECT expect('person', 'tb_user', '<unknown>');

-- The TVIEWs stay fresh through value-only and linking writes alike.
UPDATE tb_user SET bio = 'b' WHERE pk_user = 1;
UPDATE tb_comment SET created_at = now() - interval '1 day' WHERE pk_comment = 1;
UPDATE tb_post SET published = false WHERE pk_post = 2;
INSERT INTO tb_comment (pk_comment, fk_post, fk_author, content) VALUES (3, 1, 1, 'ada on pa');
SELECT assert_fresh('tv_post', 'pk_post', 'tv_post');
SELECT assert_fresh('tv_feed', 'pk_feed', 'tv_feed');
SELECT assert_fresh('tv_latest', 'pk_latest', 'tv_latest');
SELECT assert_fresh('tv_person', 'pk_person', 'tv_person');

\echo 'issue #263 link columns: PASS'
