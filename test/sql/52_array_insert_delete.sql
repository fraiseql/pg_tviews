-- This test verifies that array elements can be inserted and deleted properly

\set ON_ERROR_STOP on

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

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
    SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_create');

    -- Initial state: no comments
    SELECT
        jsonb_array_length(data->'comments') AS initial_comment_count
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 0
    DO $$ BEGIN
        IF (SELECT jsonb_path_query_array(data, '$.comments[*].author') FROM tv_post
            WHERE pk_post = 1) IS DISTINCT FROM '[]'::jsonb THEN
            RAISE EXCEPTION 'FAIL: tv_post comments not empty before any INSERT';
        END IF;
    END $$;

    -- Test 1: INSERT new comment (should add to array)
    INSERT INTO tb_comment VALUES (1, gen_random_uuid(), 1, 'Alice', 'First comment!');
    SELECT assert_fresh('tv_post', 'pk_post', 'first INSERT INTO tb_comment');

    -- Verify: 1 comment now
    SELECT
        jsonb_array_length(data->'comments') AS after_insert_count,
        data->'comments'->0->>'author' AS first_comment_author,
        data->'comments'->0->>'text' AS first_comment_text
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 1 | Alice | First comment!
    DO $$ BEGIN
        IF (SELECT data->'comments'->0->>'text' FROM tv_post WHERE pk_post = 1)
           IS DISTINCT FROM 'First comment!' THEN
            RAISE EXCEPTION 'FAIL: first tv_post comment text is not First comment!';
        END IF;
    END $$;
    DO $$ BEGIN
        IF (SELECT jsonb_path_query_array(data, '$.comments[*].author') FROM tv_post
            WHERE pk_post = 1) IS DISTINCT FROM '["Alice"]'::jsonb THEN
            RAISE EXCEPTION 'FAIL: tv_post comment authors not [Alice] after INSERT';
        END IF;
    END $$;

    -- Test 2: INSERT another comment (should append to array)
    INSERT INTO tb_comment VALUES (2, gen_random_uuid(), 1, 'Bob', 'Second comment!');
    SELECT assert_fresh('tv_post', 'pk_post', 'second INSERT INTO tb_comment');

    -- Verify: 2 comments, properly ordered
    SELECT
        jsonb_array_length(data->'comments') AS after_second_insert_count,
        data->'comments'->0->>'author' AS first_author,
        data->'comments'->1->>'author' AS second_author
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 2 | Alice | Bob
    DO $$ BEGIN
        IF (SELECT jsonb_path_query_array(data, '$.comments[*].author') FROM tv_post
            WHERE pk_post = 1) IS DISTINCT FROM '["Alice", "Bob"]'::jsonb THEN
            RAISE EXCEPTION 'FAIL: tv_post comment authors not [Alice, Bob] after second INSERT';
        END IF;
    END $$;

    -- Test 3: DELETE first comment (should remove from array)
    DELETE FROM tb_comment WHERE pk_comment = 1;
    SELECT assert_fresh('tv_post', 'pk_post', 'DELETE of the first comment');

    -- Verify: 1 comment remaining, Bob's comment
    SELECT
        jsonb_array_length(data->'comments') AS after_delete_count,
        data->'comments'->0->>'author' AS remaining_author,
        data->'comments'->0->>'text' AS remaining_text
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 1 | Bob | Second comment!
    DO $$ BEGIN
        IF (SELECT jsonb_path_query_array(data, '$.comments[*].author') FROM tv_post
            WHERE pk_post = 1) IS DISTINCT FROM '["Bob"]'::jsonb THEN
            RAISE EXCEPTION 'FAIL: tv_post comment authors not [Bob] after DELETE';
        END IF;
    END $$;
    DO $$ BEGIN
        IF (SELECT data->'comments'->0->>'text' FROM tv_post WHERE pk_post = 1)
           IS DISTINCT FROM 'Second comment!' THEN
            RAISE EXCEPTION 'FAIL: remaining tv_post comment text is not Second comment!';
        END IF;
    END $$;

    -- Test 4: DELETE last comment (should empty array)
    DELETE FROM tb_comment WHERE pk_comment = 2;
    SELECT assert_fresh('tv_post', 'pk_post', 'DELETE of the last comment');

    -- Verify: back to empty array
    SELECT
        jsonb_array_length(data->'comments') AS final_count
    FROM tv_post
    WHERE pk_post = 1;
    -- Expected: 0
    DO $$ BEGIN
        IF (SELECT jsonb_path_query_array(data, '$.comments[*].author') FROM tv_post
            WHERE pk_post = 1) IS DISTINCT FROM '[]'::jsonb THEN
            RAISE EXCEPTION 'FAIL: tv_post comments not empty after deleting every comment';
        END IF;
    END $$;

ROLLBACK;