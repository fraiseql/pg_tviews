-- Issue #280: pg_tviews_resync(source, keys) refreshes what given rows of a base
-- table reach, as an UPDATE of them would, without writing them.
--
-- After writes made with pg_tviews' triggers off (session_replication_role =
-- replica, triggers disabled, a restore of one table), the TVIEW rows those rows
-- feed are stale. Resync maps the rows'
-- current images through every TVIEW that reads the table, recomputes the rows
-- they reach (and those of the TVIEWs embedding them), and writes only the
-- documents that changed; it fires no trigger and writes no source row. A key whose
-- row is gone removes the row of every TVIEW keyed by it. The caller needs UPDATE
-- on the table, what the write itself would need; each TVIEW refreshes as its owner.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_resync.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#280 FAIL: %', what; END IF; END $$;
-- The SQLSTATE `stmt` fails with, NULL when it succeeds.
CREATE FUNCTION failure(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE; END $$;

-- User ← Post (embeds its author's TVIEW row), and a team summary mapped through
-- the membership table.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE TABLE tb_team (pk_team int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      label text NOT NULL);
CREATE TABLE tb_member (pk_member int PRIMARY KEY, fk_team int NOT NULL REFERENCES tb_team,
                        fk_user int NOT NULL);
CREATE INDEX ON tb_post (fk_user);
CREATE INDEX ON tb_member (fk_team);
CREATE INDEX ON tb_member (fk_user);
INSERT INTO tb_user (pk_user, name) SELECT g, 'u' || g FROM generate_series(1, 5) g;
INSERT INTO tb_post (pk_post, fk_user, title) SELECT g, 1 + g % 5, 'p' || g FROM generate_series(1, 20) g;
INSERT INTO tb_team (pk_team, label) VALUES (1, 'red'), (2, 'blue');
INSERT INTO tb_member VALUES (1, 1, 1), (2, 1, 2), (3, 2, 3), (4, 2, 4), (5, 2, 5);

SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('id', u.id, 'name', u.name) AS data
    FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('id', p.id, 'title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
SELECT tviews.pg_tviews_create('tv_team', $$
    SELECT t.pk_team, t.id, jsonb_build_object('label', t.label,
        'members', (SELECT jsonb_agg(u.name ORDER BY u.pk_user)
                    FROM tb_member m JOIN tb_user u ON u.pk_user = m.fk_user
                    WHERE m.fk_team = t.pk_team)) AS data
    FROM tb_team t $$);

CREATE FUNCTION in_sync() RETURNS boolean LANGUAGE sql AS $$
    SELECT NOT EXISTS (SELECT 1 FROM tv_user t FULL JOIN tviews.public__tv_user v USING (pk_user)
                       WHERE t.data IS DISTINCT FROM v.data)
       AND NOT EXISTS (SELECT 1 FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post)
                       WHERE t.data IS DISTINCT FROM v.data)
       AND NOT EXISTS (SELECT 1 FROM tv_team t FULL JOIN tviews.public__tv_team v USING (pk_team)
                       WHERE t.data IS DISTINCT FROM v.data) $$;

-- 1. Two users renamed with no trigger firing: three TVIEWs stale.
SET session_replication_role = replica;
UPDATE tb_user SET name = name || '!' WHERE pk_user IN (1, 3);
RESET session_replication_role;
SELECT must(NOT in_sync(), 'the writes without triggers refreshed');
-- Resync reports the rows it changed per TVIEW: 2 users, their 8 posts, 2 teams.
SELECT jsonb_object_agg(tview, rows_changed) AS changed
    FROM tviews.pg_tviews_resync('tb_user', ARRAY[1, 3]) \gset
SELECT must(:'changed'::jsonb = '{"public.tv_user": 2, "public.tv_post": 8, "public.tv_team": 2}',
            'rows changed: ' || :'changed');
SELECT must(in_sync(), 'out of sync after resync');

-- 2. A resync of rows already in sync changes nothing: the TVIEWs reading tb_user
-- are listed, tv_post (changed only through tv_user) is not.
SELECT jsonb_object_agg(tview, rows_changed) AS changed
    FROM tviews.pg_tviews_resync('tb_user', ARRAY[1, 2, 3, 4, 5]) \gset
SELECT must(:'changed'::jsonb = '{"public.tv_user": 0, "public.tv_team": 0}',
            'a resync in sync changed rows: ' || :'changed');

-- 3. A row deleted with no trigger firing: its TVIEW row goes.
SET session_replication_role = replica;
DELETE FROM tb_post WHERE fk_user = 5;
DELETE FROM tb_member WHERE fk_user = 5;
DELETE FROM tb_user WHERE pk_user = 5;
RESET session_replication_role;
SELECT tviews.pg_tviews_resync('tb_user', ARRAY[5]);
SELECT must(NOT EXISTS (SELECT FROM tv_user WHERE pk_user = 5), 'tv_user row 5 kept');
-- The posts and the membership are other tables: resynced by their own keys.
SELECT tviews.pg_tviews_resync('tb_post', ARRAY(SELECT pk_post FROM tv_post WHERE fk_user = 5));
SELECT tviews.pg_tviews_resync('tb_team', ARRAY[2]);
SELECT must(in_sync(), 'out of sync after resyncing deletes');

-- 4. No source row is written, no trigger fires.
CREATE TABLE fired (n int);
CREATE FUNCTION note_fired() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN INSERT INTO fired VALUES (1); RETURN NULL; END $$;
CREATE TRIGGER user_written AFTER UPDATE ON tb_user FOR EACH STATEMENT EXECUTE FUNCTION note_fired();
SELECT xmin AS before_xmin FROM tb_user WHERE pk_user = 1 \gset
SELECT tviews.pg_tviews_resync('tb_user', ARRAY[1]);
SELECT must((SELECT xmin FROM tb_user WHERE pk_user = 1)::text = :'before_xmin', 'tb_user row 1 written');
SELECT must(NOT EXISTS (SELECT FROM fired), 'a trigger fired');

-- 5. Privileges: UPDATE on the table, as the write would need.
DROP ROLE IF EXISTS resync_reader_280;
DROP ROLE IF EXISTS resync_writer_280;
CREATE ROLE resync_reader_280;
CREATE ROLE resync_writer_280;
GRANT USAGE ON SCHEMA tviews TO resync_reader_280, resync_writer_280;
GRANT SELECT ON tb_user TO resync_reader_280;
GRANT SELECT, UPDATE ON tb_user TO resync_writer_280;
SET ROLE resync_reader_280;
SELECT must(failure($$SELECT tviews.pg_tviews_resync('tb_user', ARRAY[1])$$) = '42501',
            'a role without UPDATE resynced');
RESET ROLE;
SET session_replication_role = replica;
UPDATE tb_user SET name = 'renamed' WHERE pk_user = 2;
RESET session_replication_role;
SET ROLE resync_writer_280;
SELECT tviews.pg_tviews_resync('tb_user', ARRAY[2]);
RESET ROLE;
SELECT must(in_sync(), 'out of sync after the writer''s resync');

-- 6. A table no TVIEW reads, and one without an integer primary key, are refused.
CREATE TABLE plain_t (pk_plain int PRIMARY KEY);
SELECT must(failure($$SELECT tviews.pg_tviews_resync('plain_t', ARRAY[1])$$) = '22023',
            'a table no TVIEW reads accepted');
CREATE TABLE no_key (a int);
SELECT must(failure($$SELECT tviews.pg_tviews_resync('no_key', ARRAY[1])$$) = '22023',
            'a table without a primary key accepted');

-- 7. A partition of a table a TVIEW reads resyncs its rows through the TVIEWs
-- reading the partitioned table, whatever its own column order.
CREATE TABLE tb_thing (pk_thing int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       name text) PARTITION BY RANGE (pk_thing);
CREATE TABLE tb_thing_low PARTITION OF tb_thing FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_thing_high (name text, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            pk_thing int NOT NULL);
ALTER TABLE tb_thing ATTACH PARTITION tb_thing_high FOR VALUES FROM (100) TO (200);
INSERT INTO tb_thing (pk_thing, name) VALUES (1, 'a'), (150, 'b');
SELECT tviews.pg_tviews_create('tv_thing', $$
    SELECT t.pk_thing, t.id, jsonb_build_object('id', t.id, 'name', t.name) AS data
    FROM tb_thing t $$);
SET session_replication_role = replica;
UPDATE tb_thing SET name = name || '!';
RESET session_replication_role;
SELECT COALESCE(jsonb_object_agg(tview, rows_changed), '{}') AS changed
    FROM tviews.pg_tviews_resync('tb_thing_high', ARRAY[150]) \gset
SELECT must(:'changed'::jsonb = '{"public.tv_thing": 1}', 'partition resync: ' || :'changed');
SELECT tviews.pg_tviews_resync('tb_thing_low', ARRAY[1]);
SELECT must(NOT EXISTS (SELECT 1 FROM tv_thing t FULL JOIN tviews.public__tv_thing v USING (pk_thing)
                        WHERE t.data IS DISTINCT FROM v.data), 'partitions out of sync after resync');

DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY resync_reader_280;
DROP OWNED BY resync_writer_280;
DROP ROLE resync_reader_280;
DROP ROLE resync_writer_280;
\echo 'resync: PASS'
