-- What registration derives from a definition follows PostgreSQL's query tree,
-- not the spelling of the SQL text.
--
-- (a) A column the definition also joins on is never patched into `data` alone:
--     the join changes too.
-- (b) An embedded TVIEW's document read through an alias (`'author', u.data`) is a
--     nested embed: a change to the child is patched into the parents.
-- (c) A parent whose column holding the child's key is not named fk_<child>
--     follows the child.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_lineage_analysis.sql
-- expect-output: lineage analysis: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'lineage analysis FAIL: %', what; END IF; END $$;
-- Direct patches applied so far in this session.
CREATE FUNCTION patches() RETURNS bigint LANGUAGE sql AS
    $$ SELECT (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint $$;

-- (a) tag is copied into data and joined on.
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, norm text);
INSERT INTO tb_tag VALUES (1, 'a'), (2, 'b');
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), tag text);
INSERT INTO tb_note (pk_note, tag) VALUES (1, 'a'), (2, 'zz');
SET pg_tviews.uncascaded_policy = 'warn';
SELECT pg_tviews_create('tv_note', $$SELECT n.pk_note, n.id,
    jsonb_build_object('tag', n.tag, 'tagged', t.pk_tag) AS data
    FROM tb_note n LEFT JOIN tb_tag t ON t.norm = n.tag$$);
RESET pg_tviews.uncascaded_policy;
UPDATE tb_note SET tag = 'b' WHERE pk_note = 1;
SELECT assert_fresh('tv_note', 'pk_note', '(a) a joined column changed');

-- (b) the author's document under an alias.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, title) SELECT g, 1 + g % 2, 't' || g FROM generate_series(1, 6) g;
SELECT pg_tviews_create('tv_user', $$SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user$$);
SELECT pg_tviews_create('tv_post', $$SELECT p.pk_post, p.id, p.fk_user,
    jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user$$);
SELECT patches() AS before \gset
UPDATE tb_user SET name = 'ann2' WHERE pk_user = 1;
SELECT assert_fresh('tv_post', 'pk_post', '(b) an aliased embedded document');
SELECT must(patches() - :before >= 3, format('(b) posts patched: %s', patches() - :before));

-- (c) the parent holds the user's key as author_pk.
SELECT pg_tviews_create('tv_article', $$SELECT p.pk_post AS pk_article, p.id, p.fk_user AS author_pk,
    jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user$$);
UPDATE tb_user SET name = 'bob2' WHERE pk_user = 2;
SELECT assert_fresh('tv_article', 'pk_article', '(c) a parent column named otherwise');

SELECT 'lineage analysis: PASS' AS result;
