-- Writers run at READ COMMITTED (docs/concurrency.md, ADR 0207).
--
-- A transaction snapshot cannot see what a writer waited for, so pg_tviews cannot
-- keep a TVIEW fresh from a REPEATABLE READ or SERIALIZABLE writer without failing
-- it after the fact. It refuses such a writer at once, with 0A000 and a hint,
-- before anything is written: a write to a table a TVIEW reads, every refresh
-- or rebuild call, and creating a TVIEW (its fill would miss a row committed
-- after the transaction's snapshot and before its triggers exist). Reads are not affected at any level, and neither are writes to
-- tables no TVIEW reads.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_read_committed_only.sql
-- expect-output: read_committed_only: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'D1 FAIL: %', msg; END IF; END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
CREATE TABLE tb_note (pk_note int PRIMARY KEY, body text);   -- read by no TVIEW
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann'), (2, DEFAULT, 'bob');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 'p1');
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$) \g /dev/null
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$) \g /dev/null

-- refused(sql, level): the SQLSTATE `sql` raises in a transaction at `level`.
CREATE FUNCTION refused(sql text, level text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE state text;
BEGIN
    -- The level is set by the caller's BEGIN; a subtransaction keeps it.
    BEGIN
        EXECUTE sql;
        RETURN 'ok';
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE;
        RETURN state;
    END;
END $$;

-- Every write path, at REPEATABLE READ and at SERIALIZABLE.
BEGIN ISOLATION LEVEL REPEATABLE READ;
SELECT must(refused('UPDATE tb_user SET name = ''x'' WHERE pk_user = 1', 'rr') = '0A000',
            'a row-trigger write at REPEATABLE READ was not refused');
SELECT must(refused('UPDATE tb_user SET name = ''x''', 'rr') = '0A000',
            'a multi-row write at REPEATABLE READ was not refused');
SELECT must(refused('INSERT INTO tb_post VALUES (2, DEFAULT, 2, ''p2'')', 'rr') = '0A000',
            'an insert at REPEATABLE READ was not refused');
SELECT must(refused('SELECT tviews.pg_tviews_refresh(''tv_post'')', 'rr') = '0A000',
            'pg_tviews_refresh at REPEATABLE READ was not refused');
SELECT must(refused('SELECT tviews.pg_tviews_refresh_all()', 'rr') = '0A000',
            'pg_tviews_refresh_all at REPEATABLE READ was not refused');
SELECT must(refused('SELECT tviews.pg_tviews_rebuild_all()', 'rr') = '0A000',
            'pg_tviews_rebuild_all at REPEATABLE READ was not refused');
SELECT must(refused('TRUNCATE tb_post', 'rr') = '0A000',
            'a TRUNCATE at REPEATABLE READ was not refused');
SELECT must(refused($q$SELECT tviews.pg_tviews_create('tv_note', 'SELECT pk_note, gen_random_uuid() AS id, jsonb_build_object(''b'', body) AS data FROM tb_note')$q$, 'rr') = '0A000',
            'pg_tviews_create at REPEATABLE READ was not refused');
SELECT must(refused($q$SELECT tviews.pg_tviews_create_or_replace('tv_note', 'SELECT pk_note, gen_random_uuid() AS id, jsonb_build_object(''b'', body) AS data FROM tb_note')$q$, 'rr') = '0A000',
            'pg_tviews_create_or_replace (create) at REPEATABLE READ was not refused');
SELECT must(refused($q$SELECT tviews.pg_tviews_create_or_replace('tv_user', 'SELECT pk_user, id, jsonb_build_object(''n'', name) AS data FROM tb_user')$q$, 'rr') = '0A000',
            'pg_tviews_create_or_replace (replace) at REPEATABLE READ was not refused');
-- Reads, and writes no TVIEW maintains, are not affected.
SELECT must((SELECT count(*) FROM tv_post) = 1, 'a read of tv_post at REPEATABLE READ failed');
SELECT must(refused('INSERT INTO tb_note VALUES (1, ''n'')', 'rr') = 'ok',
            'a write to a table no TVIEW reads was refused at REPEATABLE READ');
COMMIT;

BEGIN ISOLATION LEVEL SERIALIZABLE;
SELECT must(refused('UPDATE tb_user SET name = ''x'' WHERE pk_user = 1', 'ser') = '0A000',
            'a write at SERIALIZABLE was not refused');
SELECT must(refused('SELECT tviews.pg_tviews_refresh(''tv_user'')', 'ser') = '0A000',
            'pg_tviews_refresh at SERIALIZABLE was not refused');
SELECT must(refused($q$SELECT tviews.pg_tviews_create('tv_note', 'SELECT pk_note, gen_random_uuid() AS id, jsonb_build_object(''b'', body) AS data FROM tb_note')$q$, 'ser') = '0A000',
            'pg_tviews_create at SERIALIZABLE was not refused');
COMMIT;

-- The hint names the fix.
BEGIN ISOLATION LEVEL REPEATABLE READ;
DO $$
DECLARE msg text; hint text;
BEGIN
    UPDATE tb_user SET name = 'x' WHERE pk_user = 1;
    RAISE EXCEPTION 'D1 FAIL: not refused';
EXCEPTION WHEN feature_not_supported THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    PERFORM must(msg LIKE '%REPEATABLE READ%' AND hint LIKE '%READ COMMITTED%',
                 'message or hint does not name the levels: ' || msg || ' / ' || hint);
END $$;
ROLLBACK;

-- A read-only REPEATABLE READ transaction reads TVIEWs as before.
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 1) = 'ann', 'read-only RR read');
COMMIT;

-- Nothing was written, and READ COMMITTED writes as before.
SELECT must((SELECT name FROM tb_user WHERE pk_user = 1) = 'ann', 'a refused write committed');
UPDATE tb_user SET name = 'ann2' WHERE pk_user = 1;
SELECT must((SELECT data->>'author' FROM tv_post WHERE pk_post = 1) = 'ann2',
            'a READ COMMITTED write did not refresh tv_post');

\echo 'read_committed_only: PASS'
