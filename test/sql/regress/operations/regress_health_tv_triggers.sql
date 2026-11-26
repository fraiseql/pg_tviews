-- Regression test: the health check reports a user trigger or rule on a TVIEW table.
-- Each changes what a maintenance write leaves in the table, which the freshness
-- theorems assume nothing does; pg_tviews' own triggers there are not reported.
--   1. a TVIEW read by another (so pg_tviews' triggers sit on its table): OK;
--   2. a user trigger on it: WARNING naming it;
--   3. a rule on it too: both named; dropped, OK again.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_health_tv_triggers.sql
-- expect-output: health tv_triggers: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'health tv_triggers FAIL: %', what; END IF; END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int REFERENCES tb_user, title text);
INSERT INTO tb_user VALUES (1, DEFAULT, 'n');
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);

-- 1.
SELECT must((SELECT status FROM tviews.pg_tviews_check_health() WHERE component = 'tv_triggers') = 'OK',
            'pg_tviews'' own triggers reported: ' ||
            coalesce((SELECT message FROM tviews.pg_tviews_check_health() WHERE component = 'tv_triggers'), 'no component'));

-- 2.
CREATE FUNCTION shout() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN NEW.data := NEW.data || '{"x": 1}'; RETURN NEW; END $$;
CREATE TRIGGER user_shout BEFORE UPDATE ON tv_user FOR EACH ROW EXECUTE FUNCTION shout();
SELECT must((SELECT status = 'WARNING' AND message LIKE '%user_shout on public.tv_user%'
             FROM tviews.pg_tviews_check_health() WHERE component = 'tv_triggers'),
            'the user trigger: ' ||
            coalesce((SELECT message FROM tviews.pg_tviews_check_health() WHERE component = 'tv_triggers'), 'no component'));

-- 3.
CREATE RULE user_quiet AS ON DELETE TO tv_post DO INSTEAD NOTHING;
SELECT must((SELECT message LIKE '%user_shout on public.tv_user%' AND message LIKE '%rule user_quiet on public.tv_post%'
             FROM tviews.pg_tviews_check_health() WHERE component = 'tv_triggers'),
            'the rule: ' || (SELECT message FROM tviews.pg_tviews_check_health() WHERE component = 'tv_triggers'));
DROP RULE user_quiet ON tv_post;
DROP TRIGGER user_shout ON tv_user;
SELECT must((SELECT status FROM tviews.pg_tviews_check_health() WHERE component = 'tv_triggers') = 'OK',
            'still reported once dropped');

\echo 'health tv_triggers: PASS'
