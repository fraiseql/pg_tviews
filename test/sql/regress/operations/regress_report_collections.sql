-- Regression test (#244): pg_tviews_flush_and_report() reports the collections a
-- transaction changed.
--
-- The change feed names, in `c`, the parents of each reference of every row it
-- publishes, before and after the write: `Comment.fk_post` and the post. A layer
-- that runs the write itself evicts from the report, inside the transaction, so
-- the report carries the same collections, under its own rules: reset, `types`
-- (by child type), savepoints, and the child type in `invalidated_types` when the
-- parents cannot be listed. They are captured only with
-- pg_tviews.publish_changes = 'rows'; otherwise the report has no `collections`.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_report_collections.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'report collections FAIL: %', what; END IF; END $$;

-- Readable ids: uid('p1') is the uuid of post 1; expand() writes a document's
-- "p1" as that uuid.
CREATE FUNCTION uid(name text) RETURNS uuid LANGUAGE sql IMMUTABLE AS $$
    SELECT ('00000000-0000-0000-0000-' || translate(left(name, 1), 'upcn', 'abcd')
            || lpad(substr(name, 2), 11, '0'))::uuid $$;
CREATE FUNCTION expand(doc text) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE out text := doc; m text[];
BEGIN
    FOR m IN SELECT regexp_matches(doc, '"([upcn][0-9]+)"', 'g') LOOP
        out := replace(out, '"' || m[1] || '"', '"' || uid(m[1]) || '"');
    END LOOP;
    RETURN out::jsonb;
END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL UNIQUE, username text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL UNIQUE,
                      fk_author int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE TABLE tb_comment (pk_comment int PRIMARY KEY, id uuid NOT NULL UNIQUE,
                         fk_post int NOT NULL REFERENCES tb_post,
                         fk_author int NOT NULL REFERENCES tb_user, content text NOT NULL);
INSERT INTO tb_user VALUES (1, uid('u1'), 'ann'), (2, uid('u2'), 'bob');
INSERT INTO tb_post VALUES (1, uid('p1'), 1, 'one'), (2, uid('p2'), 2, 'two'),
                           (3, uid('p3'), 1, 'three');
INSERT INTO tb_comment VALUES (1, uid('c1'), 1, 2, 'first'), (2, uid('c2'), 1, 1, 'second');

SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('username', u.username) AS data FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_author $$);
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, c.fk_author,
           jsonb_build_object('content', c.content) AS data FROM tb_comment c $$);

-- A child of a TVIEW without an id column: its parents cannot be named.
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, label text NOT NULL);
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL,
                      fk_tag int NOT NULL REFERENCES tb_tag, body text NOT NULL);
INSERT INTO tb_tag VALUES (1, 'x');
SELECT tviews.pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, jsonb_build_object('label', t.label) AS data FROM tb_tag t $$);
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT n.pk_note, n.id, n.fk_tag, jsonb_build_object('body', n.body) AS data FROM tb_note n $$);

SET pg_tviews.publish_changes = rows;

-- 1. Inserted into p1: its collections, though no row of p1 changed.
BEGIN;
INSERT INTO tb_comment VALUES (3, uid('c3'), 1, 1, 'third');
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must((:'r'::jsonb)->'collections'
            = expand('{"Comment.fk_author": ["u1"], "Comment.fk_post": ["p1"]}'),
            'insert: ' || :'r');
COMMIT;

-- 2. Moved from p1 to p2: both posts.
BEGIN;
UPDATE tb_comment SET fk_post = 2 WHERE pk_comment = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must((:'r'::jsonb)->'collections'->'Comment.fk_post' = expand('["p1", "p2"]'),
            'move: ' || :'r');
COMMIT;

-- 3. Moved twice in one transaction (p2 -> p3 -> p1): every post it left or
--    joined, not only the last write's.
BEGIN;
UPDATE tb_comment SET fk_post = 3 WHERE pk_comment = 1;
UPDATE tb_comment SET fk_post = 1 WHERE pk_comment = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must((:'r'::jsonb)->'collections'->'Comment.fk_post' = expand('["p1", "p2", "p3"]'),
            'moved twice: ' || :'r');
COMMIT;

-- 4. Reset: a second report lists only what changed after the first.
BEGIN;
UPDATE tb_comment SET content = 'edited' WHERE pk_comment = 2;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must((:'r'::jsonb)->'collections'->'Comment.fk_post' = expand('["p1"]'),
            'before reset: ' || :'r');
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must((:'r'::jsonb)->'collections' = '{}', 'after reset: ' || :'r');
-- Without reset, the same collections again.
UPDATE tb_comment SET content = 'edited again' WHERE pk_comment = 2;
SELECT tviews.pg_tviews_flush_and_report(reset => false) AS r \gset
SELECT tviews.pg_tviews_flush_and_report() AS r2 \gset
SELECT must((:'r'::jsonb)->'collections' = (:'r2'::jsonb)->'collections'
            AND (:'r'::jsonb)->'collections' <> '{}', 'no reset: ' || :'r' || ' then ' || :'r2');
COMMIT;

-- 5. types filters collections by their child type.
BEGIN;
UPDATE tb_comment SET content = 'typed' WHERE pk_comment = 2;
SELECT tviews.pg_tviews_flush_and_report(types => ARRAY['Post'], reset => false) AS r \gset
SELECT must((:'r'::jsonb)->'collections' = '{}', 'types Post: ' || :'r');
SELECT tviews.pg_tviews_flush_and_report(types => ARRAY['Comment']) AS r \gset
SELECT must((:'r'::jsonb)->'collections'->'Comment.fk_post' = expand('["p1"]'),
            'types Comment: ' || :'r');
COMMIT;

-- 6. A rolled-back savepoint's writes name nothing.
BEGIN;
SAVEPOINT s;
INSERT INTO tb_comment VALUES (5, uid('c5'), 2, 2, 'rolled back');
ROLLBACK TO SAVEPOINT s;
UPDATE tb_comment SET content = 'kept' WHERE pk_comment = 2;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must((:'r'::jsonb)->'collections'
            = expand('{"Comment.fk_author": ["u1"], "Comment.fk_post": ["p1"]}'),
            'savepoint: ' || :'r');
COMMIT;

-- 7. A parent without an id: the child type is invalidated instead.
BEGIN;
INSERT INTO tb_note VALUES (1, uid('n1'), 1, 'b');
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(NOT (:'r'::jsonb)->'collections' ? 'Note.fk_tag', 'no id listed: ' || :'r');
SELECT must((:'r'::jsonb)->'invalidated_types' ? 'Note', 'no id invalidated: ' || :'r');
COMMIT;

-- 8. The report and the change feed name the same collections for one
--    transaction (the feed lists the new parent first; compared as sets).
CREATE FUNCTION sorted(c jsonb) RETURNS jsonb LANGUAGE sql AS $$
    SELECT coalesce(jsonb_object_agg(key, (SELECT jsonb_agg(v ORDER BY v)
                                           FROM jsonb_array_elements(value) v)), '{}')
    FROM jsonb_each(c) $$;
\setenv PGTV_DB :DBNAME
\set both `out=$(psql -X -q -At -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "BEGIN" -c "UPDATE tb_comment SET fk_post = 3, fk_author = 2 WHERE pk_comment = 2" -c "SELECT tviews.pg_tviews_flush_and_report()->'collections'" -c "COMMIT" -c "SELECT 1" 2>&1); echo "$out" | grep '^{' | head -1; echo '|'; echo "$out" | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/'`
SELECT split_part(:'both', '|', 1) AS reported, split_part(:'both', '|', 2) AS fed \gset
SELECT must(sorted(:'reported'::jsonb) = sorted((:'fed'::jsonb)->'c')
            AND :'reported'::jsonb <> '{}', 'report vs feed: ' || :'both');

-- 9. Rows left out past max_entities: their type is invalidated, and stands for
--    its collections (as a type in the feed's `t`).
BEGIN;
UPDATE tb_comment SET content = 'many' WHERE pk_comment IN (1, 2);
UPDATE tb_post SET title = 'many' WHERE pk_post = 1;
SELECT tviews.pg_tviews_flush_and_report(max_entities => 1) AS r \gset
SELECT must((:'r'::jsonb)->'invalidated_types' ? 'Comment', 'left out: ' || :'r');
SELECT must(NOT EXISTS (SELECT FROM jsonb_object_keys((:'r'::jsonb)->'collections') k
                        WHERE k LIKE 'Comment.%'), 'left-out collections listed: ' || :'r');
COMMIT;

-- 10. Not captured: no collections key at all ("not known" is not "none").
SET pg_tviews.publish_changes = off;
BEGIN;
UPDATE tb_comment SET content = 'unpublished' WHERE pk_comment = 2;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(NOT (:'r'::jsonb) ? 'collections', 'off: ' || :'r');
COMMIT;

-- 11. What decides is the setting when the rows were written, not when the
--     report runs. (Comment 2 is on p3 since section 8.)
BEGIN;
UPDATE tb_comment SET content = 'written off' WHERE pk_comment = 2;
SET LOCAL pg_tviews.publish_changes = rows;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(NOT (:'r'::jsonb) ? 'collections', 'written off, reported rows: ' || :'r');
SELECT must(NOT (:'r'::jsonb)->'invalidated_types' ? 'Comment', 'uncaptured invalidated: ' || :'r');
COMMIT;
BEGIN;
SET LOCAL pg_tviews.publish_changes = rows;
UPDATE tb_comment SET content = 'written rows' WHERE pk_comment = 2;
SET LOCAL pg_tviews.publish_changes = off;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must((:'r'::jsonb)->'collections'->'Comment.fk_post' = expand('["p3"]'),
            'written rows, reported off: ' || :'r');
COMMIT;

\echo 'regress_report_collections: PASS'
