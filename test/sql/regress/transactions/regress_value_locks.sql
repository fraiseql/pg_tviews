-- A write locks the join values of the rows it changed before it looks TVIEW
-- rows up by them, and a refresh the values its rows read before it computes
-- them (ADR 0207): exclusive and shared value locks, with RowExclusive and
-- RowShare intent locks on the relation, in PostgreSQL's lock manager as
-- advisory locks whose objsubid is 21622 (value) or 21623 (relation). They last until the end of the
-- transaction, go with a rolled-back savepoint, are kept by a prepared
-- transaction, can't be taken or blocked through pg_advisory_lock(), and aren't
-- taken under SERIALIZABLE.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_value_locks.sql
-- expect-output: value_locks: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'value_locks FAIL: %', what; END IF; END $$;

-- This backend's (or a prepared transaction's) pg_tviews locks on relation rel:
-- 'value:<mode>' or 'relation:<mode>', counted. Only a writer's modes, unless
-- refresh: then only a refresh's.
CREATE FUNCTION held(rel regclass, prepared boolean DEFAULT false, refresh boolean DEFAULT false)
RETURNS text[] LANGUAGE sql AS $$
    SELECT coalesce(array_agg(k || ':' || n ORDER BY k), '{}') FROM (
        SELECT CASE objsubid WHEN 21622 THEN 'value' ELSE 'relation' END || ':' || mode AS k,
               count(*) AS n
        FROM pg_locks
        WHERE locktype = 'advisory' AND objsubid IN (21622, 21623) AND classid = rel::oid
          AND (mode IN ('ShareLock', 'RowShareLock')) = refresh
          AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
          AND CASE WHEN prepared THEN pid IS NULL ELSE pid = pg_backend_pid() END
        GROUP BY 1) s $$;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann'), (2, DEFAULT, 'bob'), (3, DEFAULT, 'cy');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 'p1'), (2, DEFAULT, 2, 'p2');

-- tv_note joins tb_user directly (a mapping query).
SELECT pg_tviews_create('tv_note', $$
    SELECT p.pk_post AS pk_note, p.id, jsonb_build_object('author', upper(u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);

-- 1. A mapped UPDATE: one exclusive value lock per changed user, one intent lock.
BEGIN;
UPDATE tb_user SET name = name || '!' WHERE pk_user IN (1, 2);
SELECT must(held('tb_user') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:2}',
            'mapped UPDATE holds ' || held('tb_user')::text);
COMMIT;
SELECT must(held('tb_user') = '{}', 'locks outlived the commit: ' || held('tb_user')::text);

-- An UPDATE of no column the TVIEW reads locks nothing; a rollback releases.
BEGIN;
UPDATE tb_user SET id = id WHERE pk_user = 1;
SELECT must(held('tb_user') = '{}', 'an UPDATE of an unread column locked ' || held('tb_user')::text);
INSERT INTO tb_user VALUES (4, DEFAULT, 'di');
SELECT must(held('tb_user') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:1}',
            'an INSERT holds ' || held('tb_user')::text);
ROLLBACK;
SELECT must(held('tb_user') = '{}', 'locks outlived the rollback');

-- 2. ROLLBACK TO SAVEPOINT releases what was taken after it; taking it again works.
BEGIN;
UPDATE tb_user SET name = name || '?' WHERE pk_user = 1;
SAVEPOINT s;
UPDATE tb_user SET name = name || '?' WHERE pk_user = 2;
SELECT must(held('tb_user') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:2}',
            'before the rollback to savepoint: ' || held('tb_user')::text);
ROLLBACK TO SAVEPOINT s;
SELECT must(held('tb_user') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:1}',
            'after the rollback to savepoint: ' || held('tb_user')::text);
UPDATE tb_user SET name = name || '?' WHERE pk_user = 2;
SELECT must(held('tb_user') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:2}',
            'a value released by the savepoint was not taken again: ' || held('tb_user')::text);
COMMIT;

-- 3. Embeds: refreshing a child row locks its key in the child's table.
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
BEGIN;
UPDATE tb_user SET name = 'ann' WHERE pk_user = 1;
SELECT must(held('tv_user') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:1}',
            'an embedded child holds ' || held('tv_user')::text);
COMMIT;

-- 4. A prepared transaction keeps them until COMMIT PREPARED, and
--    pg_advisory_lock() on the same numbers neither waits nor conflicts.
BEGIN;
UPDATE tb_user SET name = 'bob' WHERE pk_user = 2;
PREPARE TRANSACTION 'value_locks';
SELECT must(held('tb_user', true) = '{relation:RowExclusiveLock:1,value:ExclusiveLock:1}',
            'the prepared transaction holds ' || held('tb_user', true)::text);
SELECT must(bool_and(pg_try_advisory_xact_lock(classid::int, objid::int)),
            'pg_advisory_lock() conflicts with a value lock')
FROM pg_locks WHERE locktype = 'advisory' AND objsubid = 21622 AND pid IS NULL;
COMMIT PREPARED 'value_locks';
SELECT must(held('tb_user', true) = '{}', 'COMMIT PREPARED left locks');

-- 5. A fan-out patch (tv_tag projects the join column and copies the name).
SELECT pg_tviews_create('tv_tag', $$
    SELECT p.pk_post AS pk_tag, p.id, p.fk_user, jsonb_build_object('who', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
BEGIN;
UPDATE tb_user SET name = 'cyd' WHERE pk_user = 3;
SELECT must(held('tb_user') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:1}',
            'a fan-out holds ' || held('tb_user')::text);
COMMIT;

-- 6. A write refreshing a whole TVIEW (full_refresh policy) locks the TVIEW.
CREATE TABLE tb_site (name text);
INSERT INTO tb_site VALUES ('blog');
SET pg_tviews.uncascaded_policy = 'full_refresh';
SELECT pg_tviews_create('tv_page', $$
    SELECT p.pk_post AS pk_page, p.id, jsonb_build_object('site', s.name) AS data
    FROM tb_post p CROSS JOIN tb_site s $$);
RESET pg_tviews.uncascaded_policy;
BEGIN;
UPDATE tb_site SET name = 'news';
SELECT must(held('tv_page') = '{relation:ExclusiveLock:1}',
            'a full refresh holds ' || held('tv_page')::text);
COMMIT;

-- 7. A partitioned table maps each row: its values are locked on the root.
CREATE TABLE tb_region (pk_region bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        label text) PARTITION BY RANGE (pk_region);
CREATE TABLE tb_region_a PARTITION OF tb_region FOR VALUES FROM (0) TO (100);
ALTER TABLE tb_post ADD COLUMN fk_region bigint;
INSERT INTO tb_region VALUES (1, DEFAULT, 'north');
UPDATE tb_post SET fk_region = 1;
SELECT pg_tviews_create('tv_area', $$
    SELECT p.pk_post AS pk_area, p.id, jsonb_build_object('region', upper(r.label)) AS data
    FROM tb_post p JOIN tb_region r ON r.pk_region = p.fk_region $$);
BEGIN;
UPDATE tb_region_a SET label = 'south' WHERE pk_region = 1;
SELECT must(held('tb_region') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:1}',
            'a partition''s row holds ' || held('tb_region')::text);
COMMIT;

-- 8. SERIALIZABLE takes none: SSI detects the conflicts.
BEGIN ISOLATION LEVEL SERIALIZABLE;
UPDATE tb_user SET name = 'ann!' WHERE pk_user = 1;
SELECT must(held('tb_user') = '{}' AND held('tv_user') = '{}',
            'SERIALIZABLE took value locks: ' || held('tb_user')::text);
COMMIT;

-- 9. A refresh locks, shared, what the rows it computes read: the post's user
--    (a value no user row may hold yet), the embedded user's key, and an intent
--    lock on each TVIEW it writes.
BEGIN;
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (9, 2, 'p9');
SELECT must(held('tb_user', refresh => true) = '{relation:RowShareLock:1,value:ShareLock:1}',
            'a refresh holds on tb_user ' || held('tb_user', refresh => true)::text);
SELECT must(held('tv_user', refresh => true) = '{relation:RowShareLock:1,value:ShareLock:1}',
            'a refresh holds on the embedded tv_user ' || held('tv_user', refresh => true)::text);
SELECT must(held('tv_post', refresh => true) = '{relation:RowShareLock:1}',
            'a refresh holds on tv_post ' || held('tv_post', refresh => true)::text);
SELECT must(held('tb_user') = '{}', 'a refresh took writer locks on tb_user');
-- ... and the key of the row it creates, exclusively, in the TVIEW's key space
-- (objid 1 for its relation lock).
SELECT must(held('tv_post') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:1}',
            'creating a row holds on tv_post ' || held('tv_post')::text);
SELECT must(EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND objsubid = 21623
                    AND classid = 'tv_post'::regclass::oid AND objid = 1
                    AND pid = pg_backend_pid()),
            'the key space''s intent lock is not on objid 1');
COMMIT;

-- 10. Escalation: past pg_tviews.lock_escalation_threshold values of one relation
--     a transaction locks the relation instead (0: always, -1: never), and
--     pg_tviews_queue_stats() counts the locks and escalations of the transaction.
INSERT INTO tb_user (pk_user, name) SELECT g, 'u' || g FROM generate_series(10, 19) g;
BEGIN;
SET LOCAL pg_tviews.lock_escalation_threshold = 4;
UPDATE tb_user SET name = name || '.' WHERE pk_user BETWEEN 10 AND 19;
SELECT must(held('tb_user') = '{relation:ExclusiveLock:1}',
            'past the threshold, a writer holds ' || held('tb_user')::text);
SELECT must((pg_tviews_queue_stats()->>'value_lock_escalations')::int >= 1,
            'no escalation counted: ' || pg_tviews_queue_stats()::text);
COMMIT;
BEGIN;
SET LOCAL pg_tviews.lock_escalation_threshold = 0;
UPDATE tb_user SET name = name || '.' WHERE pk_user = 10;
SELECT must(held('tb_user') = '{relation:ExclusiveLock:1}',
            'at threshold 0, a writer holds ' || held('tb_user')::text);
COMMIT;
BEGIN;
SET LOCAL pg_tviews.lock_escalation_threshold = -1;
UPDATE tb_user SET name = name || '.' WHERE pk_user BETWEEN 10 AND 19;
SELECT must(held('tb_user') = '{relation:RowExclusiveLock:1,value:ExclusiveLock:10}',
            'at threshold -1, a writer holds ' || held('tb_user')::text);
SELECT must((pg_tviews_queue_stats()->>'value_locks')::int >= 10
            AND (pg_tviews_queue_stats()->>'value_lock_escalations')::int = 0
            AND (pg_tviews_queue_stats()->>'value_lock_waits')::int = 0,
            'lock counters: ' || pg_tviews_queue_stats()::text);
COMMIT;

\echo 'value_locks: PASS'
