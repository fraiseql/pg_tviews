-- Every table a TVIEW maps by a query has read sets in its plan (ADR 0207): the
-- column of the table its mapping joins on, and a query from the TVIEW's keys
-- to the values that column is compared with. A refresh locks those values; a
-- writer of the table locks its rows' values of that column. They exist even
-- when no row of the table matches (an outer join to a row not inserted yet).
--
-- tviews.pg_tviews_read_set_queries(tview, table) renders them; each query is
-- run here with the keys of a row and must return the values that row reads.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_plan_read_sets.sql
-- expect-output: plan_read_sets: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'plan_read_sets FAIL: %', what; END IF; END $$;

-- The values the read sets of tview's mapping of tbl return for keys, per column.
CREATE FUNCTION read_values(tview text, tbl regclass, keys bigint[])
RETURNS TABLE(column_name text, vals text[]) LANGUAGE plpgsql AS $$
DECLARE r record;
BEGIN
    FOR r IN SELECT * FROM tviews.pg_tviews_read_set_queries(tview, tbl) LOOP
        column_name := r.column_name;
        IF r.query IS NULL THEN
            vals := NULL;
        ELSE
            EXECUTE 'SELECT array_agg(v ORDER BY v) FROM (' || r.query || ') s(v)' INTO vals USING keys;
        END IF;
        RETURN NEXT;
    END LOOP;
END $$;

CREATE TABLE tb_org (pk_org bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_org bigint REFERENCES tb_org, name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint, fk_editor bigint, title text, pos int);
CREATE TABLE tb_rank (pk_rank bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      min_pos int, label text);
INSERT INTO tb_org VALUES (1, DEFAULT, 'acme');
INSERT INTO tb_user VALUES (1, DEFAULT, 1, 'ann'), (2, DEFAULT, 1, 'bob');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 2, 'p1', 5), (2, DEFAULT, 9, NULL, 'p2', 1);
INSERT INTO tb_rank VALUES (1, DEFAULT, 3, 'top');

-- 1. One hop: the post's fk_user, read from the post.
SELECT pg_tviews_create('tv_note', $$
    SELECT p.pk_post AS pk_note, p.id, jsonb_build_object('author', upper(u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
SELECT must((SELECT array_agg(column_name || '=' || vals::text)
             FROM read_values('tv_note', 'tb_user', '{1}')) = '{"pk_user={1}"}',
            'one hop: ' || (SELECT array_agg(column_name || '=' || vals::text)
                            FROM read_values('tv_note', 'tb_user', '{1}'))::text);

-- 2. Two hops: the org of the post's user.
SELECT pg_tviews_create('tv_feed', $$
    SELECT p.pk_post AS pk_feed, p.id, jsonb_build_object('org', upper(o.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user JOIN tb_org o ON o.pk_org = u.fk_org $$);
SELECT must((SELECT vals FROM read_values('tv_feed', 'tb_org', '{1}')) = '{1}',
            'two hops: tb_org');
SELECT must((SELECT vals FROM read_values('tv_feed', 'tb_user', '{1}')) = '{1}',
            'two hops: tb_user');

-- 3. Two reads of one column: author and editor, one read set.
SELECT pg_tviews_create('tv_byline', $$
    SELECT p.pk_post AS pk_byline, p.id,
           jsonb_build_object('author', upper(a.name), 'editor', upper(e.name)) AS data
    FROM tb_post p JOIN tb_user a ON a.pk_user = p.fk_user JOIN tb_user e ON e.pk_user = p.fk_editor $$);
SELECT must((SELECT array_agg(column_name || '=' || vals::text)
             FROM read_values('tv_byline', 'tb_user', '{1}')) = '{"pk_user={1,2}"}',
            'two reads: ' || (SELECT array_agg(column_name || '=' || vals::text)
                              FROM read_values('tv_byline', 'tb_user', '{1}'))::text);

-- 4. An outer join: post 2 names user 9, who doesn't exist; 9 is still read.
SELECT pg_tviews_create('tv_draft', $$
    SELECT p.pk_post AS pk_draft, p.id, jsonb_build_object('author', u.name) AS data
    FROM tb_post p LEFT JOIN tb_user u ON u.pk_user = p.fk_user $$);
SELECT must((SELECT vals FROM read_values('tv_draft', 'tb_user', '{2}')) = '{9}',
            'outer join: ' || (SELECT vals FROM read_values('tv_draft', 'tb_user', '{2}'))::text);

-- 5. A join by no equality: the table is locked as a whole.
SELECT pg_tviews_create('tv_ranked', $$
    SELECT p.pk_post AS pk_ranked, p.id,
           jsonb_build_object('rank', (SELECT max(r.label) FROM tb_rank r WHERE r.min_pos < p.pos)) AS data
    FROM tb_post p $$);
SELECT must((SELECT count(*) FROM read_values('tv_ranked', 'tb_rank', '{1}')
             WHERE column_name IS NULL AND vals IS NULL) = 1,
            'no equality: ' || (SELECT array_agg(r::text)
                                FROM tviews.pg_tviews_read_set_queries('tv_ranked', 'tb_rank'::regclass) r)::text);

-- 6. Another TVIEW's table read like a base table (no output column holds its key).
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT pg_tviews_create('tv_card', $$
    SELECT p.pk_post AS pk_card, p.id, jsonb_build_object('who', u.data->'name') AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
SELECT must((SELECT array_agg(column_name || '=' || vals::text)
             FROM read_values('tv_card', 'tv_user', '{1}')) = '{"pk_user={1}"}',
            'a TVIEW table: ' || (SELECT array_agg(column_name || '=' || vals::text)
                                  FROM read_values('tv_card', 'tv_user', '{1}'))::text);

-- 7. Every table mapped by a query has read sets, and every one renders.
SELECT must(NOT EXISTS (
    SELECT 1 FROM tviews.pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t
    WHERE t->>'kind' = 'mapped' AND jsonb_array_length(coalesce(t->'reads', '[]')) = 0),
    'a mapped table without read sets');
SELECT must(NOT EXISTS (
    SELECT 1 FROM tviews.pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t,
         tviews.pg_tviews_read_set_queries(m.entity, (t->>'relid')::oid) q
    WHERE q.column_name IS NOT NULL AND q.query IS NULL),
    'a read set that does not render');

-- 8. A table that is not mapped by a query has none: the table holding the key.
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_read_set_queries('tv_note', 'tb_post'::regclass)),
            'the table holding the key has read sets');

\echo 'plan_read_sets: PASS'
