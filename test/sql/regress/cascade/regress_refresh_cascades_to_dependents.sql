-- pg_tviews_refresh(entity) rebuilds the TVIEW, then every TVIEW whose view reads
-- it, directly or transitively, in dependency order: a manual repair leaves nothing
-- stale. pg_tviews_refresh_all() still rebuilds each TVIEW once, and the catch-up
-- after a suspension still rebuilds what embeds a changed TVIEW.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_refresh_cascades_to_dependents.sql
--
-- expect-output: refresh cascades to dependents: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL, bio text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE TABLE tb_feed (pk_feed int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_post int NOT NULL REFERENCES tb_post);
CREATE TABLE tb_badge (pk_badge int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_user int NOT NULL REFERENCES tb_user, label text);
INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'alice', 'a'), (2, 'bob', 'b');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');
INSERT INTO tb_feed (pk_feed, fk_post) VALUES (1, 1), (2, 2);
INSERT INTO tb_badge (pk_badge, fk_user, label) VALUES (1, 1, 'gold'), (2, 2, 'silver');

-- user → post → feed, each view reading the previous TVIEW's document; badge
-- reads only tv_user's name.
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user $$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
SELECT pg_tviews_create('tv_feed', $$
    SELECT f.pk_feed, f.id, f.fk_post, jsonb_build_object('post', p.data) AS data
    FROM tb_feed f JOIN tv_post p ON p.pk_post = f.fk_post $$);
SELECT pg_tviews_create('tv_badge', $$
    SELECT b.pk_badge, b.id, b.fk_user, jsonb_build_object('label', b.label, 'who', u.data->'name') AS data
    FROM tb_badge b JOIN tv_user u ON u.pk_user = b.fk_user $$);

CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['user', 'post', 'feed', 'badge'] LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tviews.public__tv_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM tviews.public__tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d',
            entity) INTO d;
        IF d <> 0 THEN
            RAISE EXCEPTION 'item 1 FAIL: tv_% stale after %', entity, step;
        END IF;
    END LOOP;
END $$;

-- ── a change no trigger saw, repaired with pg_tviews_refresh('user') ────────
SET session_replication_role = replica;
UPDATE tb_user SET name = 'alice (renamed)', bio = 'a2' WHERE pk_user = 1;
RESET session_replication_role;
SELECT pg_tviews_refresh('user');
SELECT assert_fresh('pg_tviews_refresh(user)');

-- ── catch-up after a suspension (one transaction: suspension ends with it) ──
BEGIN;
SELECT pg_tviews_suspend_triggers();
UPDATE tb_user SET name = 'bob (renamed)' WHERE pk_user = 2;
SELECT pg_tviews_resume_triggers();
COMMIT;
SELECT assert_fresh('catch-up after resume');

-- ── refresh_all still rebuilds each TVIEW once ──────────────────────────────
DO $$
DECLARE r jsonb := tviews.pg_tviews_refresh_all();
BEGIN
    IF (r->>'refreshed_count')::int <> 4
       OR (SELECT count(DISTINCT e) FROM jsonb_array_elements_text(r->'order') e) <> 4 THEN
        RAISE EXCEPTION 'item 1 FAIL: pg_tviews_refresh_all() rebuilt %', r->'order';
    END IF;
END $$;
SELECT assert_fresh('pg_tviews_refresh_all()');

\echo 'refresh cascades to dependents: PASS'
