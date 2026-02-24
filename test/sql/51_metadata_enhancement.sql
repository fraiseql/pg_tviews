-- This test verifies that new metadata fields are populated

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

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

    -- Verify metadata row exists
    SELECT COUNT(*) = 1 AS meta_exists FROM pg_tview_meta WHERE entity = 'post';
    -- Expected: t

    -- Verify new columns exist (they should have defaults, not NULL)
    SELECT
        dependency_types IS NOT NULL AS has_dep_types_col,
        dependency_paths IS NOT NULL AS has_dep_paths_col,
        array_match_keys IS NOT NULL AS has_array_keys_col
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: t, t, t

    -- Test Case 2: Verify columns can be queried

    SELECT
        array_length(dependency_types, 1) AS dep_types_len,
        array_length(fk_columns, 1) AS fk_cols_len
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: NULL or 0 (empty), >= 1 (has FK to user)

ROLLBACK;
