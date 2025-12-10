-- Phase 5 Task 6: Array Handling Implementation
-- Test 2: JSONB Array Element Updates (RED Phase)
-- This test verifies that JSONB array elements can be updated using jsonb_smart_patch_array

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

    -- Test Case 1: JSONB array element update with smart patching
    CREATE TABLE tb_post (
        pk_post INTEGER PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid(),
        title TEXT
    );

    CREATE TABLE tb_comment (
        pk_comment INTEGER PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid(),
        fk_post INTEGER REFERENCES tb_post(pk_post),
        author TEXT,
        text TEXT
    );

    INSERT INTO tb_post VALUES (1, gen_random_uuid(), 'First Post');
    INSERT INTO tb_comment VALUES (1, gen_random_uuid(), 1, 'Alice', 'Great post!');
    INSERT INTO tb_comment VALUES (2, gen_random_uuid(), 1, 'Bob', 'Thanks for sharing!');

    -- Create TVIEW with array of comments
    SELECT pg_tviews_create('post', $$
        SELECT
            p.pk_post,
            p.id,
            p.title,
            jsonb_build_object(
                'id', p.id,
                'title', p.title,
                'comments', COALESCE(
                    jsonb_agg(
                        jsonb_build_object('id', c.id, 'author', c.author, 'text', c.text)
                        ORDER BY c.pk_comment
                    ),
                    '[]'::jsonb
                )
            ) AS data
        FROM tb_post p
        LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
        GROUP BY p.pk_post, p.id, p.title
    $$);

    -- Verify initial state
    SELECT
        jsonb_array_length(data->'comments') AS initial_comment_count,
        data->'comments'->0->>'text' AS first_comment_text,
        data->'comments'->1->>'text' AS second_comment_text
    FROM tv_post
    WHERE pk_post = 1;

    -- Expected: 2 | Great post! | Thanks for sharing!

    -- Test: Update one comment (should use jsonb_smart_patch_array)
    UPDATE tb_comment SET text = 'Updated: Great post!' WHERE pk_comment = 1;

    -- Verify: Only the updated comment changed
    SELECT
        jsonb_array_length(data->'comments') AS after_update_comment_count,
        data->'comments'->0->>'text' AS updated_first_comment,
        data->'comments'->1->>'text' AS unchanged_second_comment
    FROM tv_post
    WHERE pk_post = 1;

    -- Expected: 2 | Updated: Great post! | Thanks for sharing!
    -- Note: This will fail initially because array dependency detection isn't implemented yet

ROLLBACK;