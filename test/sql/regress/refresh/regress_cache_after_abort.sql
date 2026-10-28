-- Issue #266: a backend's caches outlive a rolled-back transaction.
--
-- Every ROLLBACK and every ROLLBACK TO SAVEPOINT cleared them, so the next write read
-- the catalog again: the mapping-query templates re-rendered, one lookup per name
-- (retries after 40001/40P01, PL/pgSQL EXCEPTION blocks and test suites that roll back
-- each test all paid it on every write). They are now cleared only when the
-- transaction rolled back a catalog change of its own: a rename or a TVIEW created
-- and rolled back is still forgotten.
--
-- pg_tviews_queue_stats() counts the transaction's own catalog lookups, so each
-- measured write runs in a transaction that reads them, and compares them with those
-- of a warm write after a commit.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_cache_after_abort.sql
-- expect-output: cache after abort: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_author (pk_author bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author bigint NOT NULL REFERENCES tb_author, title text);
INSERT INTO tb_author (pk_author, name) SELECT g, 'a' || g FROM generate_series(1, 5) g;
INSERT INTO tb_post (pk_post, fk_author, title) SELECT g, 1 + g % 5, 'p' || g FROM generate_series(1, 20) g;
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', a.name)) AS data
    FROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author $$);

CREATE FUNCTION lookups() RETURNS int LANGUAGE sql AS
$$ SELECT (tviews.pg_tviews_queue_stats()->>'catalog_lookups')::int $$;
CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'cache after abort FAIL: %', msg; END IF; END $$;
CREATE FUNCTION in_sync() RETURNS boolean LANGUAGE sql AS $$
    SELECT NOT EXISTS (SELECT 1 FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post)
                       WHERE t.data IS DISTINCT FROM v.data) $$;

-- Warm, then what a warm write reads of the catalog when nothing rolled back.
UPDATE tb_author SET name = name || '!' WHERE pk_author = 1;
BEGIN;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 1;
SELECT lookups() AS warm \gset
COMMIT;

-- ── a warm write after ROLLBACK reads no catalog ─────────────────────────────
BEGIN;
UPDATE tb_author SET name = name || '?' WHERE pk_author = 2;
ROLLBACK;
BEGIN;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 3;
SELECT must(lookups() = :warm, format('a warm write after ROLLBACK made %s catalog lookups, %s without it',
                                     lookups(), :warm));
COMMIT;

-- ── … and after ROLLBACK TO SAVEPOINT, and an error caught in PL/pgSQL ───────
BEGIN;
SAVEPOINT s;
UPDATE tb_author SET name = name || '?' WHERE pk_author = 2;
ROLLBACK TO SAVEPOINT s;
DO $$ BEGIN
    UPDATE tb_author SET name = name || '?' WHERE pk_author = 2;
    RAISE EXCEPTION 'undo';
EXCEPTION WHEN raise_exception THEN NULL;
END $$;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 4;
SELECT must(lookups() = :warm, format('a warm write after rolled-back subtransactions made %s catalog lookups, %s without them',
                                     lookups(), :warm));
COMMIT;
SELECT must(in_sync(), 'tv_post out of sync after the rolled-back writes');

-- ── a rename rolled back is forgotten ────────────────────────────────────────
BEGIN;
ALTER TABLE tb_author RENAME COLUMN name TO full_name;
UPDATE tb_author SET full_name = full_name || '?' WHERE pk_author = 2;
ROLLBACK;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 2;
SELECT must(in_sync(), 'tv_post out of sync after a rolled-back rename');
BEGIN;
SAVEPOINT s;
ALTER TABLE tb_author RENAME COLUMN name TO full_name;
UPDATE tb_author SET full_name = full_name || '?' WHERE pk_author = 5;
ROLLBACK TO SAVEPOINT s;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 5;
COMMIT;
SELECT must(in_sync(), 'tv_post out of sync after a rename rolled back to a savepoint');

-- ── a TVIEW created and rolled back is forgotten ─────────────────────────────
BEGIN;
SELECT tviews.pg_tviews_create('tv_author', $$
    SELECT pk_author, id, jsonb_build_object('name', name) AS data FROM tb_author $$);
UPDATE tb_author SET name = name || '?' WHERE pk_author = 1;
ROLLBACK;
UPDATE tb_author SET name = name || '!' WHERE pk_author = 1;
SELECT must(in_sync(), 'tv_post out of sync after a rolled-back create');
SELECT must(to_regclass('tv_author') IS NULL, 'tv_author survived its rollback');

\echo 'cache after abort: PASS'
