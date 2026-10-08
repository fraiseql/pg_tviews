-- Two READ COMMITTED transactions whose writes recompute the same TVIEW row: the
-- second waited on the first's row lock inside its upsert, then wrote the document
-- it had computed before the first committed, and the TVIEW lost the first
-- writer's change. The refresh now locks the rows first, then recomputes them
-- with a snapshot that sees the other writer's commit.
--
-- Two dblink connections stand for the two writers; this session checks. The same
-- holds for a DISTINCT ON TVIEW, whose rows are locked by their DISTINCT ON key
-- (ADR 0169).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_concurrent_refresh_read_committed.sql
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

-- Wait until the second writer's backend waits on a lock: no fixed sleep.
SELECT set_config('regress.second_pid', p::text, false)
  FROM dblink('second', 'SELECT pg_backend_pid()') AS t(p int);
CREATE FUNCTION wait_for_second_writer() RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    writer int := current_setting('regress.second_pid')::int;
BEGIN
    FOR i IN 1..1000 LOOP
        PERFORM pg_stat_clear_snapshot();
        IF EXISTS (SELECT 1 FROM pg_stat_activity
                   WHERE pid = writer AND wait_event_type = 'Lock') THEN
            RETURN;
        END IF;
        PERFORM pg_sleep(0.01);
    END LOOP;
    RAISE EXCEPTION 'the second writer never waited on a lock';
END $$;

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
        PERFORM wait_for_second_writer();
        PERFORM dblink_exec('first', 'COMMIT');
        PERFORM * FROM dblink_get_result('second') AS r(status text);
        PERFORM * FROM dblink_get_result('second') AS r(status text);
        IF (SELECT data FROM tv_post WHERE pk_post = 1)
           IS DISTINCT FROM (SELECT data FROM tviews.public__tv_post WHERE pk_post = 1) THEN
            RAISE EXCEPTION 'race FAIL (run %): tv_post has %, the view has %', run,
                (SELECT data FROM tv_post WHERE pk_post = 1),
                (SELECT data FROM tviews.public__tv_post WHERE pk_post = 1);
        END IF;
    END LOOP;
END $$;

-- A DISTINCT ON TVIEW keyed on a uuid: the latest revision of each document.
CREATE TABLE tb_rev (pk_rev bigint PRIMARY KEY, id uuid NOT NULL, rev int NOT NULL,
                     fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_rev VALUES (1, '00000000-0000-0000-0000-000000000001', 1, 1, 'r1'),
                          (2, '00000000-0000-0000-0000-000000000001', 2, 1, 'r2');
SELECT pg_tviews_create('tv_rev', $$
    SELECT DISTINCT ON (r.id) r.pk_rev, r.id, r.fk_user,
           jsonb_build_object('title', r.title, 'author', u.name) AS data
    FROM tb_rev r JOIN tb_user u ON u.pk_user = r.fk_user ORDER BY r.id, r.rev DESC $$);

DO $$
DECLARE run int;
BEGIN
    FOR run IN 1..3 LOOP
        PERFORM dblink_exec('first', format('UPDATE tb_user SET name = %L', 'old'));
        PERFORM dblink_exec('first', 'UPDATE tb_rev SET title = ''t0''');
        PERFORM dblink_exec('first', 'BEGIN');
        PERFORM dblink_exec('first', format('UPDATE tb_user SET name = %L WHERE pk_user = 1', 'D' || run));
        PERFORM dblink_send_query('second', 'UPDATE tb_rev SET title = ''T2'' WHERE pk_rev = 2');
        PERFORM wait_for_second_writer();
        PERFORM dblink_exec('first', 'COMMIT');
        PERFORM * FROM dblink_get_result('second') AS r(status text);
        PERFORM * FROM dblink_get_result('second') AS r(status text);
        IF (SELECT data FROM tv_rev) IS DISTINCT FROM (SELECT data FROM tviews.public__tv_rev) THEN
            RAISE EXCEPTION 'race FAIL (DISTINCT ON, run %): tv_rev has %, the view has %', run,
                (SELECT data FROM tv_rev), (SELECT data FROM tviews.public__tv_rev);
        END IF;
    END LOOP;
END $$;

SELECT dblink_disconnect('first');
SELECT dblink_disconnect('second');

\echo 'concurrent refresh under READ COMMITTED: PASS'
