-- A write fails loudly when the row trigger cannot read the propagation plans.
--
-- The row trigger of a base table finds the TVIEW keys a changed row holds in
-- the stored plans. When a plan cannot be decoded (a catalog edited by hand, a
-- restore out of step), the write raises an ERROR naming the TVIEW, with the
-- hint to re-register it. It never commits with nothing queued: that would
-- leave every TVIEW over the table silently stale.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_plan_unreadable_fails_write.sql
-- expect-output: unreadable plan: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
CREATE TABLE tb_tag (pk_tag bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     label text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
INSERT INTO tb_tag (pk_tag, label) VALUES (1, 'red');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$$);
SELECT pg_tviews_create('tv_tag', $$
    SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM tb_tag
$$);

-- The plan of tv_tag no longer decodes.
UPDATE tviews.pg_tview_meta SET plan = jsonb_set(plan, '{paths}', '"garbage"')
 WHERE entity = 'tag';

-- The health check names it.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check()
                   WHERE status = 'ERROR' AND component = 'plans'
                     AND message LIKE '%tv_tag%' AND message LIKE '%pg_tviews_reregister%') THEN
        RAISE EXCEPTION 'FAIL: the health check does not report the unreadable plan: %',
            (SELECT string_agg(status || ' ' || component || ': ' || message, '; ')
               FROM tviews.pg_tviews_health_check());
    END IF;
END $$;

-- A write to a table only tv_user reads: the trigger reads every plan to find
-- the paths of tb_user, so it cannot tell what to refresh.
DO $$
DECLARE
    msg text;
    hint text;
BEGIN
    UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
    RAISE EXCEPTION 'FAIL: a write succeeded while the propagation plans are unreadable';
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    IF msg LIKE 'FAIL:%' THEN
        RAISE;
    END IF;
    IF msg NOT LIKE '%tv_tag%' OR hint NOT LIKE '%pg_tviews_reregister%' THEN
        RAISE EXCEPTION 'FAIL: the error does not name the TVIEW and the fix: % (hint: %)',
            msg, hint;
    END IF;
END $$;

-- Nothing was written: tv_user still matches tb_user.
DO $$
BEGIN
    IF (SELECT name FROM tb_user WHERE pk_user = 1) <> 'ada'
       OR (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) <> 'ada' THEN
        RAISE EXCEPTION 'FAIL: tv_user and tb_user disagree';
    END IF;
END $$;

-- Re-registering derives the plan again: writes refresh the TVIEWs.
SELECT tviews.pg_tviews_reregister('tag');
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check() WHERE status <> 'OK'
                                                               AND component = 'plans') THEN
        RAISE EXCEPTION 'FAIL: the health check still reports a plan after re-registration';
    END IF;
END $$;
UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
UPDATE tb_tag SET label = 'blue' WHERE pk_tag = 1;
DO $$
BEGIN
    IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) <> 'grace'
       OR (SELECT data->>'label' FROM tv_tag WHERE pk_tag = 1) <> 'blue' THEN
        RAISE EXCEPTION 'FAIL: a TVIEW is stale after re-registration';
    END IF;
END $$;

-- A plan that decodes but cannot be followed fails the same way: a table whose
-- writes map through a query, stored without the query.
CREATE TABLE tb_team (pk_team bigint PRIMARY KEY, title text);
CREATE TABLE tb_member (pk_member bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        fk_team bigint REFERENCES tb_team);
INSERT INTO tb_team VALUES (1, 'core');
INSERT INTO tb_member (pk_member, fk_team) VALUES (1, 1);
SELECT pg_tviews_create('tv_member', $$
    SELECT m.pk_member, m.id, jsonb_build_object('team', t.title) AS data
    FROM tb_member m JOIN tb_team t ON t.pk_team = m.fk_team
$$);
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta, jsonb_array_elements(plan->'tables') t
                   WHERE entity = 'member' AND t->>'table' = 'public.tb_team'
                     AND t->>'kind' = 'mapped') THEN
        RAISE EXCEPTION 'FAIL: tb_team is not mapped through a query';
    END IF;
END $$;
UPDATE tviews.pg_tview_meta m
   SET plan = jsonb_set(plan, ARRAY['tables', i::text], (plan->'tables'->i) - 'sql')
  FROM generate_series(0, 1) i
 WHERE m.entity = 'member' AND plan->'tables'->i->>'table' = 'public.tb_team';
DO $$
DECLARE
    msg text;
    hint text;
BEGIN
    UPDATE tb_team SET title = 'platform' WHERE pk_team = 1;
    RAISE EXCEPTION 'FAIL: a write through a mapping with no query succeeded';
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    IF msg LIKE 'FAIL:%' THEN
        RAISE;
    END IF;
    IF msg NOT LIKE '%tv_member%' OR hint NOT LIKE '%pg_tviews_reregister%' THEN
        RAISE EXCEPTION 'FAIL: the error does not name the TVIEW and the fix: % (hint: %)',
            msg, hint;
    END IF;
END $$;
SELECT tviews.pg_tviews_reregister('member');
UPDATE tb_team SET title = 'platform' WHERE pk_team = 1;
DO $$
BEGIN
    IF (SELECT data->>'team' FROM tv_member WHERE pk_member = 1) <> 'platform' THEN
        RAISE EXCEPTION 'FAIL: tv_member stale after re-registration';
    END IF;
END $$;

\echo 'unreadable plan: PASS'
