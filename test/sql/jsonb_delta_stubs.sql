-- Stub implementation of jsonb_delta functions for performance testing
-- These implement the same interface but with simplified logic

-- Drop existing if any
DROP FUNCTION IF EXISTS jsonb_smart_patch_nested(jsonb, jsonb, text[]) CASCADE;
DROP FUNCTION IF EXISTS jsonb_smart_patch_array(jsonb, jsonb, text[], text) CASCADE;
DROP FUNCTION IF EXISTS jsonb_smart_patch_array(jsonb, jsonb, text, text, jsonb) CASCADE;
DROP FUNCTION IF EXISTS jsonb_smart_patch_scalar(jsonb, jsonb) CASCADE;

-- Nested object patching: merges patch at specific path
CREATE OR REPLACE FUNCTION jsonb_smart_patch_nested(
    data jsonb,
    patch jsonb,
    path text[]
) RETURNS jsonb
LANGUAGE plpgsql IMMUTABLE
AS $$
DECLARE
    result jsonb;
    path_expr text;
BEGIN
    -- Build path expression: data #> path
    -- Then merge: (data #> path) || patch
    -- Then set back: jsonb_set(data, path, merged)

    IF array_length(path, 1) = 1 THEN
        -- Single level: {path[1]: patch}
        result := jsonb_set(
            data,
            path,
            COALESCE(data -> path[1], '{}'::jsonb) || patch,
            true
        );
    ELSIF array_length(path, 1) = 2 THEN
        -- Two levels: {path[1]: {path[2]: patch}}
        result := jsonb_set(
            data,
            path,
            COALESCE(data #> path, '{}'::jsonb) || patch,
            true
        );
    ELSE
        -- Generic case for arbitrary depth
        result := jsonb_set(
            data,
            path,
            COALESCE(data #> path, '{}'::jsonb) || patch,
            true
        );
    END IF;

    RETURN result;
END;
$$;

-- Array patching: updates the element matching (match_key = match_value) in the
-- array at array_path. Signature mirrors the SHIPPED jsonb_delta 0.1.0 exactly
-- (target, source, array_path TEXT scalar, match_key TEXT, match_value jsonb) so
-- the stub can never again mask a signature mismatch (issue #50).
CREATE OR REPLACE FUNCTION jsonb_smart_patch_array(
    target jsonb,
    source jsonb,
    array_path text,
    match_key text,
    match_value jsonb
) RETURNS jsonb
LANGUAGE plpgsql IMMUTABLE
AS $$
DECLARE
    result jsonb;
    array_data jsonb;
    element jsonb;
    path text[];
    idx int;
BEGIN
    path := string_to_array(array_path, '.');
    array_data := target #> path;
    result := target;

    IF array_data IS NOT NULL AND jsonb_typeof(array_data) = 'array' THEN
        FOR idx IN 0..jsonb_array_length(array_data) - 1 LOOP
            element := array_data -> idx;
            IF element -> match_key = match_value THEN
                -- Merge the source document into the matching element.
                result := jsonb_set(result, path || ARRAY[idx::text], element || source, false);
                EXIT;
            END IF;
        END LOOP;
    END IF;

    RETURN result;
END;
$$;

-- Scalar patching: shallow merge at top level
CREATE OR REPLACE FUNCTION jsonb_smart_patch_scalar(
    data jsonb,
    patch jsonb
) RETURNS jsonb
LANGUAGE plpgsql IMMUTABLE
AS $$
BEGIN
    -- Simple shallow merge
    RETURN data || patch;
END;
$$;

-- Create extension check function for testing
CREATE OR REPLACE FUNCTION jsonb_delta_available() RETURNS boolean
LANGUAGE sql IMMUTABLE
AS $$
    SELECT true; -- Always return true since we have stubs
$$;

COMMENT ON FUNCTION jsonb_smart_patch_nested IS 'Stub implementation for testing - merges patch at nested path';
COMMENT ON FUNCTION jsonb_smart_patch_array IS 'Stub implementation for testing - updates array element by match key';
COMMENT ON FUNCTION jsonb_smart_patch_scalar IS 'Stub implementation for testing - shallow merge';