-- Keys that reach a TVIEW from columns of another type than its identity's, and a
-- TVIEW whose tb_<entity> table is only joined (ADR 0169):
-- - a key read off a domain over bigint propagated nothing to the TVIEWs
--   embedding the refreshed rows;
-- - a TVIEW keyed on a DISTINCT ON uuid, joined to tb_<entity>, had writes to
--   tb_<entity> keyed as pk_<entity> integers, and the flush failed on the uuid
--   cast.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/identity/regress_identity_key_sources.sql
--
-- expect-output: identity key sources: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- ── a domain over bigint ─────────────────────────────────────────────────────
CREATE DOMAIN id_t AS bigint;
CREATE TABLE tb_author (pk_author id_t PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_book (pk_book bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author id_t NOT NULL REFERENCES tb_author, title text);
INSERT INTO tb_author VALUES (1, default, 'a'), (2, default, 'b');
INSERT INTO tb_book VALUES (1, default, 1, 't1'), (2, default, 2, 't2');
SELECT pg_tviews_create('tv_author', $$
  SELECT pk_author, id, jsonb_build_object('name', name) AS data FROM tb_author $$);
SELECT pg_tviews_create('tv_book', $$
  SELECT b.pk_book, b.id, b.fk_author, jsonb_build_object('title', b.title, 'author', a.data) AS data
  FROM tb_book b JOIN tv_author a ON a.pk_author = b.fk_author $$);
UPDATE tb_author SET name = 'a2' WHERE pk_author = 1;
SELECT assert_fresh('tv_book', 'pk_book', 'an UPDATE of a domain-keyed child');
UPDATE tb_author SET name = name || '!';
SELECT assert_fresh('tv_book', 'pk_book', 'a two-row UPDATE of a domain-keyed child');

-- ── tb_<entity> only joined to a DISTINCT ON TVIEW ──────────────────────────
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL UNIQUE, name text);
CREATE TABLE tb_signup (pk_signup int PRIMARY KEY, user_uuid uuid NOT NULL, at int NOT NULL);
INSERT INTO tb_user VALUES (7, '00000000-0000-0000-0000-000000000007', 'u7');
INSERT INTO tb_signup VALUES (1, '00000000-0000-0000-0000-000000000007', 1),
                             (2, '00000000-0000-0000-0000-000000000007', 2);
SELECT pg_tviews_create('tv_user', $$
  SELECT DISTINCT ON (s.user_uuid) s.user_uuid AS id, u.pk_user,
         jsonb_build_object('name', u.name, 'at', s.at) AS data
  FROM tb_signup s JOIN tb_user u ON u.id = s.user_uuid ORDER BY s.user_uuid, s.at DESC $$);
UPDATE tb_user SET name = 'renamed';
SELECT assert_fresh('tv_user', 'id', 'an UPDATE of the joined tb_user');
INSERT INTO tb_signup VALUES (3, '00000000-0000-0000-0000-000000000007', 3);
SELECT assert_fresh('tv_user', 'id', 'a new signup');

\echo 'identity key sources: PASS'
