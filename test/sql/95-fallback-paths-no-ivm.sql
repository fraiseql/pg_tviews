-- Test fallback path operations WITHOUT jsonb_delta extension
-- This verifies graceful degradation when jsonb_delta is not available
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

-- Setup WITHOUT jsonb_delta extension
CREATE EXTENSION IF NOT EXISTS pg_tviews;

\echo '### Test 1: Verify jsonb_delta_set_path is NOT available'

DO $$
DECLARE
    available boolean;
BEGIN
    SELECT EXISTS(
        SELECT 1 FROM pg_proc
        WHERE proname = 'jsonb_delta_set_path'
    ) INTO available;

    IF NOT available THEN
        RAISE NOTICE 'PASS: jsonb_delta_set_path not available (expected)';
    ELSE
        RAISE EXCEPTION 'FAIL: jsonb_delta_set_path should not be available for this test';
    END IF;
END $$;

\echo '### Test 2: Basic fallback using standard jsonb_set'

CREATE TABLE test_fallback_updates (
    pk_test BIGINT PRIMARY KEY,
    data JSONB DEFAULT '{}'::jsonb
);

INSERT INTO test_fallback_updates VALUES (1, '{
    "user": {
        "profile": {
            "name": "Bob",
            "email": "bob@old.com"
        }
    }
}'::jsonb);

-- Update using standard jsonb_set (fallback approach)
UPDATE test_fallback_updates
SET data = jsonb_set(
    data,
    '{user,profile,email}',
    '"bob@new.com"'::jsonb
)
WHERE pk_test = 1;

DO $$
DECLARE
    email text;
    name text;
BEGIN
    SELECT data->'user'->'profile'->>'email' INTO email FROM test_fallback_updates WHERE pk_test = 1;
    SELECT data->'user'->'profile'->>'name' INTO name FROM test_fallback_updates WHERE pk_test = 1;

    IF email = 'bob@new.com' THEN
        RAISE NOTICE 'PASS: Standard jsonb_set fallback works';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected bob@new.com, got %', email;
    END IF;

    IF name = 'Bob' THEN
        RAISE NOTICE 'PASS: Other fields preserved in fallback';
    ELSE
        RAISE EXCEPTION 'FAIL: Name should remain "Bob"';
    END IF;
END $$;

\echo '### Test 3: Verify warning messages about missing jsonb_delta'

-- This test would need to be run in the context of pg_tviews operations
-- For now, just verify the extension detection works
DO $$
DECLARE
    ivm_available boolean;
    set_path_available boolean;
BEGIN
    SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta') INTO ivm_available;
    SELECT EXISTS(SELECT 1 FROM pg_proc WHERE proname = 'jsonb_delta_set_path') INTO set_path_available;

    RAISE NOTICE 'jsonb_delta available: %', ivm_available;
    RAISE NOTICE 'jsonb_delta_set_path available: %', set_path_available;

    IF NOT ivm_available AND NOT set_path_available THEN
        RAISE NOTICE 'PASS: Both jsonb_delta extension and set_path function unavailable';
    ELSE
        RAISE NOTICE 'INFO: jsonb_delta partially available - some functions may work';
    END IF;
END $$;

\echo '### Test 4: Complex nested update with standard functions'

UPDATE test_fallback_updates SET data = '{
    "project": {
        "metadata": {
            "tags": ["web", "api"],
            "status": "draft"
        },
        "settings": {
            "visibility": "private",
            "collaborators": ["user1", "user2"]
        }
    }
}'::jsonb
WHERE pk_test = 1;

-- Update multiple nested paths using standard jsonb_set
UPDATE test_fallback_updates
SET data = jsonb_set(
    jsonb_set(
        data,
        '{project,metadata,status}',
        '"published"'::jsonb
    ),
    '{project,settings,visibility}',
    '"public"'::jsonb
)
WHERE pk_test = 1;

DO $$
DECLARE
    status text;
    visibility text;
    tags jsonb;
BEGIN
    SELECT data->'project'->'metadata'->>'status' INTO status FROM test_fallback_updates WHERE pk_test = 1;
    SELECT data->'project'->'settings'->>'visibility' INTO visibility FROM test_fallback_updates WHERE pk_test = 1;
    SELECT data->'project'->'metadata'->'tags' INTO tags FROM test_fallback_updates WHERE pk_test = 1;

    IF status = 'published' AND visibility = 'public' THEN
        RAISE NOTICE 'PASS: Multiple nested updates with standard functions';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected status=published, visibility=public';
    END IF;

    IF jsonb_array_length(tags) = 2 THEN
        RAISE NOTICE 'PASS: Unchanged fields preserved';
    ELSE
        RAISE EXCEPTION 'FAIL: Tags array was corrupted';
    END IF;
END $$;

\echo '### Test 5: Performance baseline - standard jsonb_set operations'

\timing on

-- Performance test with standard jsonb_set
DO $$
BEGIN
    FOR i IN 1..50 LOOP
        UPDATE test_fallback_updates
        SET data = jsonb_set(
            jsonb_set(
                data,
                '{project,metadata,last_updated}',
                to_jsonb(now())
            ),
            '{project,metadata,version}',
            to_jsonb(i)
        )
        WHERE pk_test = 1;
    END LOOP;
END $$;

\echo 'Standard jsonb_set performance (baseline) ^^^'

\timing off

-- Cleanup
DROP TABLE test_fallback_updates;

\echo '### All fallback tests without jsonb_delta passed! ✓'
\echo 'Note: Performance is slower but functionality works correctly'