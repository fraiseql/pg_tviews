-- Test nested path array updates - jsonb_delta_array_update_where_path
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

-- Setup test schema
CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Test 1: Direct nested path array element update
\echo '### Test 1: Direct nested path array element update'

CREATE TABLE test_nested_arrays (
    pk_test BIGINT PRIMARY KEY,
    data JSONB DEFAULT '{
        "items": [
            {"id": 1, "metadata": {"tags": [{"name": "urgent", "color": "red"}]}},
            {"id": 2, "metadata": {"tags": [{"name": "normal", "color": "blue"}]}}
        ]
    }'::jsonb
);

INSERT INTO test_nested_arrays VALUES (1);

-- Update color of tag in first item
UPDATE test_nested_arrays
SET data = jsonb_delta_array_update_where_path(
    data,
    'items',
    'id',
    '1'::jsonb,
    'metadata.tags[0].color',
    '"green"'::jsonb
)
WHERE pk_test = 1;

-- Verify
DO $$
DECLARE
    updated_color text;
BEGIN
    SELECT data->'items'->0->'metadata'->'tags'->0->>'color' INTO updated_color
    FROM test_nested_arrays WHERE pk_test = 1;

    IF updated_color = 'green' THEN
        RAISE NOTICE 'PASS: Deep nested path updated correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected "green", got %', updated_color;
    END IF;
END $$;

-- Test 2: Multiple nested updates
\echo '### Test 2: Multiple nested updates'

-- Update name in second item
UPDATE test_nested_arrays
SET data = jsonb_delta_array_update_where_path(
    data,
    'items',
    'id',
    '2'::jsonb,
    'metadata.tags[0].name',
    '"important"'::jsonb
)
WHERE pk_test = 1;

-- Verify both updates
DO $$
DECLARE
    first_color text;
    second_name text;
BEGIN
    SELECT data->'items'->0->'metadata'->'tags'->0->>'color' INTO first_color
    FROM test_nested_arrays WHERE pk_test = 1;

    SELECT data->'items'->1->'metadata'->'tags'->0->>'name' INTO second_name
    FROM test_nested_arrays WHERE pk_test = 1;

    IF first_color = 'green' AND second_name = 'important' THEN
        RAISE NOTICE 'PASS: Multiple nested updates work correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected green/important, got %/%', first_color, second_name;
    END IF;
END $$;

-- Test 3: TVIEW integration with nested path cascade
\echo '### Test 3: TVIEW integration with nested path cascade'

-- Create source tables
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    name TEXT,
    email TEXT
);

CREATE TABLE tb_comment (
    pk_comment BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    fk_user BIGINT REFERENCES tb_user(pk_user),
    text TEXT
);

CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    title TEXT
);

-- Insert test data
INSERT INTO tb_user (name, email) VALUES ('Alice', 'alice@example.com');
INSERT INTO tb_comment (fk_user, text) VALUES (1, 'Great post!');
INSERT INTO tb_post (title) VALUES ('Test Post');

-- Create TVIEW with nested author in comments array
CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    p.title,
    jsonb_build_object(
        'comments', COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', c.id,
                    'text', c.text,
                    'author', jsonb_build_object(
                        'id', u.id,
                        'name', u.name,
                        'email', u.email
                    )
                )
            )
            FROM tb_comment c
            JOIN tb_user u ON c.fk_user = u.pk_user
            WHERE c.fk_user IS NOT NULL  -- Only comments with authors
        ), '[]'::jsonb)
    ) AS data,
    now() AS created_at,
    now() AS updated_at
FROM tb_post p;

-- Test cascade: Update user name → Should update nested author.name in TVIEW
UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;

-- The cascade should use nested path update if metadata supports it
-- For now, verify the TVIEW was updated (even if not surgically)
DO $$
DECLARE
    author_name text;
BEGIN
    SELECT data->'comments'->0->'author'->>'name' INTO author_name
    FROM tv_post WHERE pk_post = 1;

    IF author_name = 'Alice Updated' THEN
        RAISE NOTICE 'PASS: TVIEW cascade with nested path works';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected "Alice Updated", got %', author_name;
    END IF;
END $$;

-- Cleanup
DROP TABLE tv_post;
DROP TABLE tb_post;
DROP TABLE tb_comment;
DROP TABLE tb_user;
DROP TABLE test_nested_arrays;

\echo '### All nested path array tests passed! ✓'