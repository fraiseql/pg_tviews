-- Regression test: the change feed's collection keys (payload v2, `c`).
--
-- A cached result that composes the comments of post P is tagged
-- `Comment.fk_post:<P's id>`. Every published Comment row (inserted, updated or
-- deleted) names the parents of each of its references, before and after the
-- write, so the cache evicts exactly the composed results it changed: an insert or
-- a move into P, which no row tag can evict, included. Notifications are read by
-- a second psql that LISTENs in its own session.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_publish_collections.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'collections FAIL: %', what; END IF; END $$;

-- Readable ids: uid('p1') is the uuid of post 1; expand() writes a payload's
-- "p1" as that uuid.
CREATE FUNCTION uid(name text) RETURNS uuid LANGUAGE sql IMMUTABLE AS $$
    SELECT ('00000000-0000-0000-0000-' || translate(left(name, 1), 'upcn', 'abcd')
            || lpad(substr(name, 2), 11, '0'))::uuid $$;
CREATE FUNCTION expand(payload text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE out text := payload; m text[];
BEGIN
    FOR m IN SELECT regexp_matches(payload, '"([upcn][0-9]+)"', 'g') LOOP
        out := replace(out, '"' || m[1] || '"', '"' || uid(m[1]) || '"');
    END LOOP;
    RETURN out;
END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL UNIQUE, username text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL UNIQUE,
                      fk_author int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE TABLE tb_comment (pk_comment int PRIMARY KEY, id uuid NOT NULL UNIQUE,
                         fk_post int NOT NULL REFERENCES tb_post ON DELETE CASCADE,
                         fk_author int NOT NULL REFERENCES tb_user, content text NOT NULL);
INSERT INTO tb_user VALUES (1, uid('u1'), 'ann'), (2, uid('u2'), 'bob');
INSERT INTO tb_post VALUES (1, uid('p1'), 1, 'one'), (2, uid('p2'), 2, 'two');
INSERT INTO tb_comment VALUES (1, uid('c1'), 1, 2, 'first'), (2, uid('c2'), 1, 1, 'second');

SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('username', u.username) AS data FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_author $$);
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, c.fk_author,
           jsonb_build_object('content', c.content,
                              'author', jsonb_build_object('username', u.username)) AS data
    FROM tb_comment c JOIN tb_user u ON u.pk_user = c.fk_author $$);

-- A child of a TVIEW without an id column: its parents cannot be named.
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, label text NOT NULL);
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL,
                      fk_tag int NOT NULL REFERENCES tb_tag, body text NOT NULL);
INSERT INTO tb_tag VALUES (1, 'x');
INSERT INTO tb_note VALUES (1, uid('n1'), 1, 'b');
SELECT tviews.pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, jsonb_build_object('label', t.label) AS data FROM tb_tag t $$);
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT n.pk_note, n.id, n.fk_tag, jsonb_build_object('body', n.body) AS data FROM tb_note n $$);

\setenv PGTV_DB :DBNAME

-- 1. Inserted into p1: its collection, though no row of p1 changed.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "INSERT INTO tb_comment VALUES (3, uid('c3'), 1, 1, 'third')" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"u":{"Comment":["c3"]},"c":{"Comment.fk_author":["u1"],"Comment.fk_post":["p1"]}}'),
            'insert: ' || :'notes');

-- 2. Moved from p1 to p2: both collections, the new parent first.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "UPDATE tb_comment SET fk_post = 2 WHERE pk_comment = 1" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"u":{"Comment":["c1"]},"c":{"Comment.fk_author":["u2"],"Comment.fk_post":["p2","p1"]}}'),
            'move: ' || :'notes');

-- 3. Edited: the collections it is in.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "UPDATE tb_comment SET content = 'edited' WHERE pk_comment = 1" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"u":{"Comment":["c1"]},"c":{"Comment.fk_author":["u2"],"Comment.fk_post":["p2"]}}'),
            'edit: ' || :'notes');

-- 4. Deleted: the collections it was in.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "DELETE FROM tb_comment WHERE pk_comment = 2" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"d":{"Comment":["c2"]},"c":{"Comment.fk_author":["u1"],"Comment.fk_post":["p1"]}}'),
            'delete: ' || :'notes');

-- 5. A rename fanned out to a post and a comment: the collections of both.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "UPDATE tb_user SET username = 'bobby' WHERE pk_user = 2" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"u":{"Comment":["c1"],"Post":["p2"],"User":["u2"]},"c":{"Comment.fk_author":["u2"],"Comment.fk_post":["p2"],"Post.fk_author":["u2"]}}'),
            'rename: ' || :'notes');

-- 6. A parent created in the same transaction is named once the flush has it.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "BEGIN" -c "INSERT INTO tb_post VALUES (3, uid('p3'), 1, 'three')" -c "INSERT INTO tb_comment VALUES (4, uid('c4'), 3, 1, 'fourth')" -c "COMMIT" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"u":{"Comment":["c4"],"Post":["p3"]},"c":{"Comment.fk_author":["u1"],"Comment.fk_post":["p3"],"Post.fk_author":["u1"]}}'),
            'new parent: ' || :'notes');

-- 7. A post deleted with its comments: the comments, deleted first, name its
--    collection; its row tag evicts its results anyway.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "DELETE FROM tb_post WHERE pk_post = 3" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"d":{"Comment":["c4"],"Post":["p3"]},"c":{"Comment.fk_author":["u1"],"Comment.fk_post":["p3"],"Post.fk_author":["u1"]}}'),
            'post deleted: ' || :'notes');

-- 8. A parent without an id column: the child type by name.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "UPDATE tb_note SET body = 'c' WHERE pk_note = 1" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"t":["Note"]}'), 'parent without id: ' || :'notes');

-- 9. `types` mode: no collection.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = types" -c "UPDATE tb_comment SET content = 'again' WHERE pk_comment = 1" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"t":["Comment"]}'), 'types mode: ' || :'notes');

-- 10. Past the cap the child type is named, and none of its collections.
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "INSERT INTO tb_comment SELECT g, uid('c' || g), 1 + g % 2, 1 + g % 2, 'bulk' FROM generate_series(100, 2099) g" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/'`
CREATE TEMP TABLE notes AS
SELECT n AS raw, n::jsonb AS payload FROM regexp_split_to_table(:'notes', E'\n') n WHERE n <> '';
SELECT must((SELECT count(*) FROM notes) BETWEEN 2 AND 8, (SELECT count(*) FROM notes) || ' messages');
SELECT must(bool_and(octet_length(raw) < 8000 AND payload->>'v' = '2'), 'over the limit or not v2')
FROM notes;
SELECT must((SELECT count(*) FROM notes WHERE payload->'t' ? 'Comment') = 1, 'Comment not named');
SELECT must(NOT EXISTS (SELECT 1 FROM notes, jsonb_object_keys(COALESCE(payload->'c', '{}')) k
                        WHERE k LIKE 'Comment.%'),
            'collections of a type named by name');

-- 11. Two TVIEWs named by tb_user.pk_user: the collection's parent is the one the
--     registry lists (identity named like the key), here the one with an id.
SELECT tviews.pg_tviews_create('tv_account', $$
    SELECT u.pk_user AS pk_account, jsonb_build_object('username', u.username) AS data
    FROM tb_user u $$);
\set notes `psql -X -q -d "$PGTV_DB" -c "LISTEN tviews_changes" -c "SET pg_tviews.publish_changes = rows" -c "UPDATE tb_comment SET content = 'tie' WHERE pk_comment = 3" -c "SELECT 1" 2>&1 | grep 'Asynchronous notification' | sed -E 's/.*with payload "(.*)" received from.*/\1/' | paste -sd '|'`
SELECT must(:'notes' = expand('{"v":2,"u":{"Comment":["c3"]},"c":{"Comment.fk_author":["u1"],"Comment.fk_post":["p1"]}}'),
            'tie-break: ' || :'notes');

\echo 'publish collections: PASS'
