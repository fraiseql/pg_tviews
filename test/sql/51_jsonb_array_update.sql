-- This test verifies that JSONB array elements can be updated using jsonb_smart_patch_array

\set ON_ERROR_STOP on

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

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
    SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_create');

    -- Verify initial state
    SELECT
        jsonb_array_length(data->'comments') AS initial_comment_count,
        data->'comments'->0->>'text' AS first_comment_text,
        data->'comments'->1->>'text' AS second_comment_text
    FROM tv_post
    WHERE pk_post = 1;

    -- Expected: 2 | Great post! | Thanks for sharing!
    DO $$ BEGIN
        IF (SELECT jsonb_path_query_array(data, '$.comments[*].text') FROM tv_post
            WHERE pk_post = 1) IS DISTINCT FROM '["Great post!", "Thanks for sharing!"]'::jsonb THEN
            RAISE EXCEPTION 'FAIL: tv_post initial comments are not the two seeded texts';
        END IF;
    END $$;

    -- Test: Update one comment (should use jsonb_smart_patch_array)
    UPDATE tb_comment SET text = 'Updated: Great post!' WHERE pk_comment = 1;
    SELECT assert_fresh('tv_post', 'pk_post', 'UPDATE tb_comment');

    -- Verify: Only the updated comment changed
    SELECT
        jsonb_array_length(data->'comments') AS after_update_comment_count,
        data->'comments'->0->>'text' AS updated_first_comment,
        data->'comments'->1->>'text' AS unchanged_second_comment
    FROM tv_post
    WHERE pk_post = 1;

    -- Expected: 2 | Updated: Great post! | Thanks for sharing!
    DO $$ BEGIN
        IF (SELECT jsonb_path_query_array(data, '$.comments[*].text') FROM tv_post
            WHERE pk_post = 1)
           IS DISTINCT FROM '["Updated: Great post!", "Thanks for sharing!"]'::jsonb THEN
            RAISE EXCEPTION 'FAIL: tv_post comments not [updated first, unchanged second]';
        END IF;
    END $$;

ROLLBACK;