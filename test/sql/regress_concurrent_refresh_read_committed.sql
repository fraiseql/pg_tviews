-- Two READ COMMITTED transactions whose writes recompute the same TVIEW row: the
-- second waited on the first's row lock inside its upsert, then wrote the document
-- it had computed before the first committed, and the TVIEW lost the first
-- writer's change. The refresh now locks the rows first, then recomputes them
-- with a snapshot that sees the other writer's commit.
--
-- Two dblink connections stand for the two writers; this session checks.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_concurrent_refresh_read_committed.sql
--
-- expect-output: concurrent refresh under READ COMMITTED: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE EXTENSION IF NOT EXISTS dblink;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'old');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 't0');
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);

-- The connections reach this database over TCP on localhost, as the same user.
SELECT format('host=localhost dbname=%s user=%s port=%s', current_database(), current_user,
              current_setting('port')) AS conninfo \gset
SELECT dblink_connect('first', :'conninfo');
SELECT dblink_connect('second', :'conninfo');

DO $$
DECLARE run int;
BEGIN
    FOR run IN 1..3 LOOP
        PERFORM dblink_exec('first', format('UPDATE tb_user SET name = %L', 'old'));
        PERFORM dblink_exec('first', 'UPDATE tb_post SET title = ''t0''');
        -- The first writer renames the author and holds its transaction open:
        -- its flush has written tv_post row 1, which it keeps locked.
        PERFORM dblink_exec('first', 'BEGIN');
        PERFORM dblink_exec('first', format('UPDATE tb_user SET name = %L WHERE pk_user = 1', 'N' || run));
        -- The second edits the post (a full recompute, not a direct patch) and
        -- blocks on that row.
        PERFORM dblink_exec('second', 'SET pg_tviews.direct_patch_enabled = off');
        PERFORM dblink_send_query('second', 'UPDATE tb_post SET title = ''T2'' WHERE pk_post = 1');
        PERFORM pg_sleep(0.3);
        PERFORM dblink_exec('first', 'COMMIT');
        PERFORM * FROM dblink_get_result('second') AS r(status text);
        PERFORM * FROM dblink_get_result('second') AS r(status text);
        IF (SELECT data FROM tv_post WHERE pk_post = 1)
           IS DISTINCT FROM (SELECT data FROM v_post WHERE pk_post = 1) THEN
            RAISE EXCEPTION 'race FAIL (run %): tv_post has %, the view has %', run,
                (SELECT data FROM tv_post WHERE pk_post = 1),
                (SELECT data FROM v_post WHERE pk_post = 1);
        END IF;
    END LOOP;
END $$;

SELECT dblink_disconnect('first');
SELECT dblink_disconnect('second');

\echo 'concurrent refresh under READ COMMITTED: PASS'
