-- A table with a virtual generated column: an UPDATE of a column that links no
-- TVIEW row is no link change (#281 follow-up).
--
-- Transition tables hold NULL for virtual generated columns, so a write to such a
-- table counted as a link change whatever it updated, and each later write of the
-- transaction took the read-set value locks a link change calls for (ADR 0207,
-- #263). Only a virtual column among the links needs that now: stored links
-- compare as they are. Compared here with the same table without the column.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_virtual_column_links.sql
-- expect-output: virtual column links: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

SELECT current_setting('server_version_num')::int >= 180000 AS pg18 \gset
\if :pg18
\else
\echo 'virtual column links: PASS (no virtual generated columns before PostgreSQL 18)'
\quit
\endif

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'virtual column links FAIL: %', what; END IF; END $$;
CREATE FUNCTION locks() RETURNS bigint LANGUAGE sql AS $$
    SELECT (tviews.pg_tviews_show_queue()->>'value_locks')::bigint $$;

-- User ← Post ← Tag, twice: tags are mapped to their user through the post, and one
-- copy's tag has a virtual generated column.
CREATE FUNCTION make(suffix text, tag_extra text) RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
    EXECUTE format('CREATE TABLE tb_user%1$s (pk_user%1$s int PRIMARY KEY,
                        id uuid NOT NULL DEFAULT gen_random_uuid(), name text NOT NULL)', suffix);
    EXECUTE format('CREATE TABLE tb_post%1$s (pk_post%1$s int PRIMARY KEY,
                        id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_user%1$s int NOT NULL REFERENCES tb_user%1$s)', suffix);
    EXECUTE format('CREATE TABLE tb_tag%1$s (pk_tag%1$s int PRIMARY KEY,
                        fk_post%1$s int NOT NULL REFERENCES tb_post%1$s, label text NOT NULL%2$s)',
                   suffix, tag_extra);
    EXECUTE format('CREATE INDEX ON tb_post%1$s (fk_user%1$s)', suffix);
    EXECUTE format('CREATE INDEX ON tb_tag%1$s (fk_post%1$s)', suffix);
    EXECUTE format('INSERT INTO tb_user%1$s (pk_user%1$s, name)
                    SELECT g, ''u'' || g FROM generate_series(1, 3) g', suffix);
    EXECUTE format('INSERT INTO tb_post%1$s (pk_post%1$s, fk_user%1$s)
                    SELECT g, 1 + g %% 3 FROM generate_series(1, 9) g', suffix);
    EXECUTE format('INSERT INTO tb_tag%1$s (pk_tag%1$s, fk_post%1$s, label)
                    SELECT g, 1 + g %% 9, ''t'' || g FROM generate_series(1, 27) g', suffix);
    PERFORM tviews.pg_tviews_create(format('tv_user%s', suffix), format($q$
        SELECT u.pk_user%1$s, u.id, jsonb_build_object('name', u.name,
            'tags', (SELECT jsonb_agg(t.label ORDER BY t.pk_tag%1$s)
                     FROM tb_post%1$s p JOIN tb_tag%1$s t ON t.fk_post%1$s = p.pk_post%1$s
                     WHERE p.fk_user%1$s = u.pk_user%1$s)) AS data
        FROM tb_user%1$s u $q$, suffix));
END $f$;
SELECT make('', '');
SELECT make('_v', ', label_len int GENERATED ALWAYS AS (length(label)) VIRTUAL');
SELECT must(cascade_kinds->>'tb_tag_v' = 'mapped', 'tb_tag_v is not mapped: ' || cascade_kinds::text)
    FROM tviews.registry WHERE name = 'tv_user_v';

-- The same two label updates in each (no link changes): the second takes the
-- read-set locks only after a link change.
BEGIN;
SELECT locks() AS l0 \gset
UPDATE tb_tag SET label = label || '!' WHERE pk_tag = 1;
UPDATE tb_tag SET label = label || '!' WHERE pk_tag = 2;
SELECT locks() - :l0 AS stored \gset
COMMIT;
BEGIN;
SELECT locks() AS l0 \gset
UPDATE tb_tag_v SET label = label || '!' WHERE pk_tag_v = 1;
UPDATE tb_tag_v SET label = label || '!' WHERE pk_tag_v = 2;
SELECT locks() - :l0 AS with_virtual \gset
COMMIT;
SELECT must(:with_virtual = :stored,
            format('with a virtual column %s value locks, %s without', :with_virtual, :stored));
-- A link change still counts: a tag moved to another post.
BEGIN;
SELECT locks() AS l0 \gset
UPDATE tb_tag_v SET fk_post_v = 3 WHERE pk_tag_v = 1;
UPDATE tb_tag_v SET label = label || '!' WHERE pk_tag_v = 2;
SELECT locks() - :l0 AS moved \gset
COMMIT;
SELECT must(:moved > :with_virtual, format('a moved tag took %s value locks', :moved));
SELECT must(NOT EXISTS (SELECT 1 FROM tv_user_v t
                        FULL JOIN tviews.public__tv_user_v v USING (pk_user_v)
                        WHERE t.data IS DISTINCT FROM v.data), 'tv_user_v out of sync');

DROP EXTENSION pg_tviews CASCADE;
\echo 'virtual column links: PASS'
