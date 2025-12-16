-- Test jsonb_delta_set_path fallback functionality
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

-- Setup
CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

\echo '### Test 1: Basic path-based update'

CREATE TABLE test_path_updates (
    pk_test BIGINT PRIMARY KEY,
    data JSONB DEFAULT '{}'::jsonb
);

INSERT INTO test_path_updates VALUES (1, '{
    "user": {
        "profile": {
            "name": "Alice",
            "email": "alice@old.com",
            "settings": {
                "theme": "light",
                "notifications": true
            }
        }
    }
}'::jsonb);

-- Update nested path
UPDATE test_path_updates
SET data = jsonb_delta_set_path(
    data,
    'user.profile.email',
    '"alice@new.com"'::jsonb
)
WHERE pk_test = 1;

DO $$
DECLARE
    email text;
    theme text;
BEGIN
    SELECT data->'user'->'profile'->>'email' INTO email FROM test_path_updates WHERE pk_test = 1;
    SELECT data->'user'->'profile'->'settings'->>'theme' INTO theme FROM test_path_updates WHERE pk_test = 1;

    IF email = 'alice@new.com' THEN
        RAISE NOTICE 'PASS: Path update succeeded';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected alice@new.com, got %', email;
    END IF;

    IF theme = 'light' THEN
        RAISE NOTICE 'PASS: Other fields preserved';
    ELSE
        RAISE EXCEPTION 'FAIL: Theme should remain "light"';
    END IF;
END $$;

\echo '### Test 2: Deep nested path with array index'

UPDATE test_path_updates SET data = '{
    "items": [
        {
            "id": 1,
            "metadata": {
                "tags": ["tag1", "tag2"],
                "status": "active"
            }
        }
    ]
}'::jsonb
WHERE pk_test = 1;

-- Update deep path with array index
UPDATE test_path_updates
SET data = jsonb_delta_set_path(
    data,
    'items[0].metadata.status',
    '"inactive"'::jsonb
)
WHERE pk_test = 1;

DO $$
DECLARE
    status text;
    tags jsonb;
BEGIN
    SELECT data->'items'->0->'metadata'->>'status' INTO status FROM test_path_updates WHERE pk_test = 1;
    SELECT data->'items'->0->'metadata'->'tags' INTO tags FROM test_path_updates WHERE pk_test = 1;

    IF status = 'inactive' THEN
        RAISE NOTICE 'PASS: Deep path with array index updated';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected "inactive", got %', status;
    END IF;

    IF jsonb_array_length(tags) = 2 THEN
        RAISE NOTICE 'PASS: Sibling fields in array element preserved';
    ELSE
        RAISE EXCEPTION 'FAIL: Tags array was modified';
    END IF;
END $$;

\echo '### Test 3: Multiple path updates (chained)'

UPDATE test_path_updates SET data = '{
    "config": {
        "server": "prod",
        "port": 8080,
        "ssl": true
    }
}'::jsonb
WHERE pk_test = 1;

-- Chain multiple path updates
UPDATE test_path_updates
SET data = jsonb_delta_set_path(
    jsonb_delta_set_path(
        jsonb_delta_set_path(
            data,
            'config.server',
            '"staging"'::jsonb
        ),
        'config.port',
        '9090'::jsonb
    ),
    'config.ssl',
    'false'::jsonb
)
WHERE pk_test = 1;

DO $$
DECLARE
    server text;
    port int;
    ssl boolean;
BEGIN
    SELECT data->'config'->>'server' INTO server FROM test_path_updates WHERE pk_test = 1;
    SELECT (data->'config'->>'port')::int INTO port FROM test_path_updates WHERE pk_test = 1;
    SELECT (data->'config'->>'ssl')::boolean INTO ssl FROM test_path_updates WHERE pk_test = 1;

    IF server = 'staging' AND port = 9090 AND ssl = false THEN
        RAISE NOTICE 'PASS: Multiple chained path updates succeeded';
    ELSE
        RAISE EXCEPTION 'FAIL: Chained updates failed';
    END IF;
END $$;

\echo '### Test 4: Creating intermediate paths'

UPDATE test_path_updates SET data = '{}'::jsonb WHERE pk_test = 1;

-- Set path that doesn't exist yet (creates intermediate objects)
UPDATE test_path_updates
SET data = jsonb_delta_set_path(
    data,
    'new.nested.deep.value',
    '"created"'::jsonb
)
WHERE pk_test = 1;

DO $$
DECLARE
    value text;
BEGIN
    SELECT data->'new'->'nested'->'deep'->>'value' INTO value FROM test_path_updates WHERE pk_test = 1;

    IF value = 'created' THEN
        RAISE NOTICE 'PASS: Intermediate paths created automatically';
    ELSE
        RAISE EXCEPTION 'FAIL: Path creation failed';
    END IF;
END $$;

\echo '### Test 5: Performance comparison - set_path vs jsonb_set'

\timing on

-- Using jsonb_set (requires multiple nested calls)
DO $$
BEGIN
    FOR i IN 1..100 LOOP
        UPDATE test_path_updates
        SET data = jsonb_set(
            jsonb_set(
                jsonb_set(
                    data,
                    '{user,name}',
                    to_jsonb('User ' || i)
                ),
                '{user,id}',
                to_jsonb(i)
            ),
            '{user,updated}',
            to_jsonb(now())
        )
        WHERE pk_test = 1;
    END LOOP;
END $$;

\echo 'jsonb_set (nested calls) ^^^'

-- Using jsonb_delta_set_path
DO $$
BEGIN
    FOR i IN 1..100 LOOP
        UPDATE test_path_updates
        SET data = jsonb_delta_set_path(
            jsonb_delta_set_path(
                jsonb_delta_set_path(
                    data,
                    'user.name',
                    to_jsonb('User ' || i)
                ),
                'user.id',
                to_jsonb(i)
            ),
            'user.updated',
            to_jsonb(now())
        )
        WHERE pk_test = 1;
    END LOOP;
END $$;

\echo 'jsonb_delta_set_path (dot notation) ^^^'
\echo 'Note: set_path should be ~2× faster'

\timing off

-- Cleanup
DROP TABLE test_path_updates;

\echo '### All fallback path tests passed! ✓'