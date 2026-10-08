-- This test verifies that new metadata fields are populated

\set ON_ERROR_STOP on

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

    -- Test Case 1: Create TVIEW and verify metadata includes new fields
    CREATE TABLE tb_user (pk_user INT PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid(), name TEXT);
    INSERT INTO tb_user VALUES (1, gen_random_uuid(), 'Alice');

    CREATE TABLE tb_post (
        pk_post INT PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid(),
        fk_user INT REFERENCES tb_user(pk_user),
        title TEXT
    );
    INSERT INTO tb_post VALUES (1, gen_random_uuid(), 1, 'First Post');

    -- Create TVIEW with nested object (user data embedded in post)
    SELECT pg_tviews_create('post', $$
        SELECT
            p.pk_post,
            p.id,
            p.fk_user,
            jsonb_build_object(
                'title', p.title,
                'author', jsonb_build_object('name', u.name)
            ) AS data
        FROM tb_post p
        LEFT JOIN tb_user u ON p.fk_user = u.pk_user
    $$);
    SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_create');

    -- Verify metadata row exists
    SELECT COUNT(*) = 1 AS meta_exists FROM pg_tview_meta WHERE entity = 'post';
    -- Expected: t
    DO $$ BEGIN
        IF (SELECT COUNT(*) FROM pg_tview_meta WHERE entity = 'post') <> 1 THEN
            RAISE EXCEPTION 'FAIL: no pg_tview_meta row for post';
        END IF;
    END $$;

    -- Verify the plan is stored, versioned
    SELECT
        (plan->>'version')::int = 1 AS plan_v1,
        jsonb_typeof(plan->'tables') = 'array' AS has_tables,
        jsonb_typeof(plan->'paths') = 'array' AS has_paths
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: t, t, t
    DO $$ BEGIN
        IF NOT (SELECT (plan->>'version')::int = 1
                       AND jsonb_typeof(plan->'tables') = 'array'
                       AND jsonb_typeof(plan->'paths') = 'array'
                FROM pg_tview_meta WHERE entity = 'post') THEN
            RAISE EXCEPTION 'FAIL: post plan is not a version 1 plan with tables and paths';
        END IF;
    END $$;

    -- Test Case 2: tb_user is mapped through tb_post, not embedded

    SELECT
        jsonb_array_length(plan->'embeds') AS embeds,
        (SELECT e->>'kind' FROM jsonb_array_elements(plan->'tables') e
         WHERE e->>'table' LIKE '%tb_user') AS tb_user_kind
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: 0, mapped
    DO $$
    DECLARE embeds int; kind text;
    BEGIN
        SELECT jsonb_array_length(plan->'embeds'),
               (SELECT e->>'kind' FROM jsonb_array_elements(plan->'tables') e
                WHERE e->>'table' LIKE '%tb_user')
          INTO embeds, kind
          FROM pg_tview_meta WHERE entity = 'post';
        IF embeds IS DISTINCT FROM 0 OR kind IS DISTINCT FROM 'mapped' THEN
            RAISE EXCEPTION 'FAIL: post plan embeds=%, tb_user kind=% (want 0, mapped)', embeds, kind;
        END IF;
    END $$;

    -- The mapping is what the flush follows: renaming the user refreshes the post.
    UPDATE tb_user SET name = 'Alice B.' WHERE pk_user = 1;
    SELECT assert_fresh('tv_post', 'pk_post', 'UPDATE tb_user');
    DO $$ BEGIN
        IF (SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'Alice B.' THEN
            RAISE EXCEPTION 'FAIL: tv_post author not refreshed by UPDATE tb_user';
        END IF;
    END $$;

ROLLBACK;
