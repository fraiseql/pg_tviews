-- Regression test (#124): TVIEWs are refreshed dependencies first.
--
-- EntityDepGraph::topo_order came out dependents-first, and the flush regrouped
-- the sorted keys into a HashMap, so the order was effectively random. That
-- matters when a TVIEW's view reads another materialized tv_* table and both have
-- pending keys in one flush: tv_post was refreshed from the old tv_user row, then
-- tv_user changed and propagation skipped tv_post as already processed.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_124_dependency_order.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    edits   int  NOT NULL DEFAULT 0
);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int  NOT NULL REFERENCES tb_user,
    title   text NOT NULL
);
CREATE TABLE tb_feed (
    pk_feed int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post int  NOT NULL REFERENCES tb_post
);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');
INSERT INTO tb_feed (pk_feed, fk_post) VALUES (1, 1), (2, 2);

-- Created in reverse alphabetical order of their dependencies, and each view reads
-- the previous TVIEW's materialized table, not its backing view.
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'edits', edits) AS data
    FROM tb_user $$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
SELECT pg_tviews_create('tv_feed', $$
    SELECT f.pk_feed, f.id, f.fk_post,
           jsonb_build_object('post', p.data) AS data
    FROM tb_feed f JOIN tv_post p ON p.pk_post = f.fk_post $$);

-- An application trigger: editing a post bumps its author's counter, so one
-- statement leaves both post and user keys pending in the same flush.
CREATE FUNCTION bump_author() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_user SET edits = edits + 1 WHERE pk_user = NEW.fk_user;
    RETURN NULL;
END $$;
CREATE TRIGGER zz_bump_author AFTER UPDATE ON tb_post
    FOR EACH ROW EXECUTE FUNCTION bump_author();

CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['user', 'post', 'feed'] LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM v_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM v_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d',
            entity) INTO d;
        IF d <> 0 THEN
            RAISE EXCEPTION '#124 FAIL after %: tv_% is stale (% rows differ from v_%)',
                step, entity, d, entity;
        END IF;
    END LOOP;
END $$;

-- (1) Flush order. The old order was random per flush, so repeat it.
DO $$
BEGIN
    FOR i IN 1..12 LOOP
        UPDATE tb_post SET title = 'p1 v' || i WHERE pk_post = 1;
        PERFORM assert_fresh(format('post update %s', i));
    END LOOP;
END $$;

-- Several posts of both authors in one statement.
UPDATE tb_post SET title = title || '!';
SELECT assert_fresh('multi-row post update');

-- The bumped counter reached the top of the chain.
DO $$ BEGIN
    IF (SELECT data->'post'->'author'->>'edits' FROM tv_feed WHERE pk_feed = 1)::int
       <> (SELECT edits FROM tb_user WHERE pk_user = 1) THEN
        RAISE EXCEPTION '#124 FAIL: tv_feed does not carry the latest author edits';
    END IF;
END $$;

-- (2) pg_tviews_refresh_all rebuilds every TVIEW, dependencies first.
DO $$
DECLARE r jsonb;
BEGIN
    r := pg_tviews_refresh_all();
    IF r->'order' <> '["user", "post", "feed"]'::jsonb THEN
        RAISE EXCEPTION '#124 FAIL: pg_tviews_refresh_all order is %', r->'order';
    END IF;
    IF (r->>'refreshed_count')::int <> 3 THEN
        RAISE EXCEPTION '#124 FAIL: pg_tviews_refresh_all refreshed % TVIEWs', r->>'refreshed_count';
    END IF;
END $$;
SELECT assert_fresh('pg_tviews_refresh_all');

-- (3) Resuming after suspended writes rebuilds dependencies first too.
BEGIN;
SELECT pg_tviews_suspend_triggers();
UPDATE tb_post SET title = 'suspended' WHERE pk_post = 2;   -- bumps bob too
UPDATE tb_feed SET fk_post = 2 WHERE pk_feed = 1;
SELECT pg_tviews_resume_triggers();
COMMIT;
SELECT assert_fresh('suspend, resume');

SELECT pg_tviews_refresh_all_entities();
SELECT assert_fresh('pg_tviews_refresh_all_entities');

SELECT 'issue #124 dependency order: PASS' AS result;
-- expect-output: issue #124 dependency order: PASS
