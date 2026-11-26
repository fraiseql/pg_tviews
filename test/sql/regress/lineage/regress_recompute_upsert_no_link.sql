-- A write that changes no link takes no shared value lock (#263), also when it
-- recomputes an embedded TVIEW's rows instead of patching them. The recompute
-- writes the child's rows with INSERT ... ON CONFLICT DO UPDATE, which fires the
-- child's statement-level INSERT trigger over an empty transition table: that
-- inserted nothing, so it changes no link of the parents reading the child.
-- Counted as an insert, it made every later refresh of the transaction lock its
-- read set shared, and two value-only writers deadlocked (40P01) in the
-- concurrent fuzzer's `values` kind with the direct patch off.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_recompute_upsert_no_link.sql
--
-- expect-output: recompute upsert no link: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, bio text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author bigint NOT NULL REFERENCES tb_user, title text);
CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post bigint NOT NULL REFERENCES tb_post,
                         fk_author bigint NOT NULL REFERENCES tb_user,
                         content text, score numeric NOT NULL DEFAULT 0);
INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'ada', ''), (2, 'bob', '');
INSERT INTO tb_post (pk_post, fk_author, title) VALUES (1, 1, 'pa'), (2, 2, 'pb');
INSERT INTO tb_comment (pk_comment, fk_post, fk_author, content)
VALUES (1, 2, 1, 'ada on pb'), (2, 1, 2, 'bob on pa');

SELECT tviews.pg_tviews_create('tv_author', $$
  SELECT u.pk_user AS pk_author, u.id, jsonb_build_object('name', u.name, 'bio', u.bio) AS data
  FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_remark', $$
  SELECT c.pk_comment AS pk_remark, c.id, c.fk_post, c.fk_author,
         jsonb_build_object('content', c.content, 'score', c.score, 'author', a.data) AS data
  FROM tb_comment c JOIN tv_author a ON a.pk_author = c.fk_author $$);
SELECT tviews.pg_tviews_create('tv_article', $$
  SELECT p.pk_post AS pk_article, p.id, p.fk_author,
         jsonb_build_object('title', p.title, 'author', a.data,
           'remarks', COALESCE((SELECT jsonb_agg(r.data ORDER BY r.pk_remark)
                                FROM tv_remark r WHERE r.fk_post = p.pk_post), '[]')) AS data
  FROM tb_post p JOIN tv_author a ON a.pk_author = p.fk_author $$);

-- The shared value locks this backend holds (ADR 0207's tag).
CREATE FUNCTION shared_value_locks() RETURNS bigint LANGUAGE sql AS $$
  SELECT count(*) FROM pg_locks
  WHERE locktype = 'advisory' AND objsubid = 21622 AND mode = 'ShareLock'
    AND pid = pg_backend_pid() $$;

CREATE FUNCTION expect_none(label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE n bigint := shared_value_locks();
BEGIN
    IF n <> 0 THEN
        RAISE EXCEPTION 'recompute upsert FAIL: %: % shared value lock(s), expected none', label, n;
    END IF;
END $$;

-- ── shipped default: a patch never copies a numeric, so the remark recomputes ─
BEGIN;
UPDATE tb_comment SET score = 2.50 WHERE pk_comment = 1;
SELECT expect_none('numeric column, direct patch on');
COMMIT;

-- ── the direct patch off: every write recomputes ─────────────────────────────
SET pg_tviews.direct_patch_enabled = off;
BEGIN;
UPDATE tb_comment SET content = 'bob, edited' WHERE pk_comment = 2;
SELECT expect_none('content column, direct patch off');
COMMIT;
BEGIN;
UPDATE tb_user SET bio = 'b' WHERE pk_user = 2;
SELECT expect_none('embedded author, direct patch off');
COMMIT;
RESET pg_tviews.direct_patch_enabled;

-- A link change still locks: a new remark inserts a row tv_article reads.
BEGIN;
INSERT INTO tb_comment (pk_comment, fk_post, fk_author, content) VALUES (3, 1, 1, 'ada on pa');
DO $$ BEGIN
    IF shared_value_locks() = 0 THEN
        RAISE EXCEPTION 'recompute upsert FAIL: an insert took no shared value lock';
    END IF;
END $$;
COMMIT;

SELECT assert_fresh('tv_author', 'pk_author', 'tv_author');
SELECT assert_fresh('tv_remark', 'pk_remark', 'tv_remark');
SELECT assert_fresh('tv_article', 'pk_article', 'tv_article');

\echo 'recompute upsert no link: PASS'
