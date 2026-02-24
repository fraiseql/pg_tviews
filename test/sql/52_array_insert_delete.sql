-- This test verifies that array elements can be inserted and deleted properly

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

    -- Test Case 1: Array element INSERT operation
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
                    ) FILTER (WHERE c.pk_comment IS NOT NULL),
                    '[]'::jsonb
                )
            ) AS data
        FROM tb_post p
        LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
        GROUP BY p.pk_post, p.id, p.title
    $$);

    -- Initial state: no comments
    SELECT
        jsonb_array_length(data->'comments') AS initial_comment_count
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 0

    -- Test 1: INSERT new comment (should add to array)
    INSERT INTO tb_comment VALUES (1, gen_random_uuid(), 1, 'Alice', 'First comment!');

    -- Verify: 1 comment now
    SELECT
        jsonb_array_length(data->'comments') AS after_insert_count,
        data->'comments'->0->>'author' AS first_comment_author,
        data->'comments'->0->>'text' AS first_comment_text
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 1 | Alice | First comment!

    -- Test 2: INSERT another comment (should append to array)
    INSERT INTO tb_comment VALUES (2, gen_random_uuid(), 1, 'Bob', 'Second comment!');

    -- Verify: 2 comments, properly ordered
    SELECT
        jsonb_array_length(data->'comments') AS after_second_insert_count,
        data->'comments'->0->>'author' AS first_author,
        data->'comments'->1->>'author' AS second_author
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 2 | Alice | Bob

    -- Test 3: DELETE first comment (should remove from array)
    DELETE FROM tb_comment WHERE pk_comment = 1;

    -- Verify: 1 comment remaining, Bob's comment
    SELECT
        jsonb_array_length(data->'comments') AS after_delete_count,
        data->'comments'->0->>'author' AS remaining_author,
        data->'comments'->0->>'text' AS remaining_text
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 1 | Bob | Second comment!

    -- Test 4: DELETE last comment (should empty array)
    DELETE FROM tb_comment WHERE pk_comment = 2;

    -- Verify: back to empty array
    SELECT
        jsonb_array_length(data->'comments') AS final_count
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 0

ROLLBACK;