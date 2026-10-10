-- A TVIEW that embeds another one twice finds its rows through both reads.
--
-- tv_doc embeds tv_user as its author and as its editor. Its plan must record
-- both lookup columns (fk_author and fk_editor): a refresh of a user then
-- refreshes the documents they wrote and those they edited. With one lookup
-- column per child, renaming the editor left tv_doc stale.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_plan_embed_twice.sql
-- expect-output: embed twice: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
CREATE TABLE tb_doc (
    pk_doc bigint PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_author bigint REFERENCES tb_user,
    fk_editor bigint REFERENCES tb_user,
    title text
);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'grace'), (3, 'edsger');
INSERT INTO tb_doc (pk_doc, fk_author, fk_editor, title) VALUES (1, 1, 2, 'one'), (2, 2, 3, 'two');

SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$$);
SELECT pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, d.fk_author, d.fk_editor,
           jsonb_build_object('title', d.title, 'author', a.data, 'editor', e.data) AS data
    FROM tb_doc d
    JOIN tv_user a ON a.pk_user = d.fk_author
    JOIN tv_user e ON e.pk_user = d.fk_editor
$$);

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
    IF EXISTS (SELECT pk_doc, data FROM tv_doc
               EXCEPT SELECT pk_doc, data FROM tviews.public__tv_doc) THEN
        RAISE EXCEPTION 'FAIL: tv_doc stale after %: %', label,
            (SELECT jsonb_agg(data ORDER BY pk_doc) FROM tv_doc);
    END IF;
END $f$;

UPDATE tb_user SET name = 'edsger w.' WHERE pk_user = 3;   -- an editor only
SELECT check_fresh('renaming an editor');
UPDATE tb_user SET name = 'ada l.' WHERE pk_user = 1;      -- an author only
SELECT check_fresh('renaming an author');
UPDATE tb_user SET name = 'grace h.' WHERE pk_user = 2;    -- both
SELECT check_fresh('renaming an author and editor');

\echo 'embed twice: PASS'
