-- Stub implementation of jsonb_ivm functions for performance testing
-- These implement the same interface but with simplified logic

-- Drop existing if any
DROP FUNCTION IF EXISTS jsonb_smart_patch_nested(jsonb, jsonb, text[]) CASCADE;
DROP FUNCTION IF EXISTS jsonb_smart_patch_array(jsonb, jsonb, text[], text) CASCADE;
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

-- Array patching: updates matching element in array at path
CREATE OR REPLACE FUNCTION jsonb_smart_patch_array(
    data jsonb,
    patch jsonb,
    path text[],
    match_key text DEFAULT 'id'
) RETURNS jsonb
LANGUAGE plpgsql IMMUTABLE
AS $$
DECLARE
    result jsonb;
    array_data jsonb;
    element jsonb;
    match_value jsonb;
    idx int;
BEGIN
    -- Get the array at the specified path
    array_data := data #> path;

    -- Get the match value from patch
    match_value := patch -> match_key;

    -- Find and update the matching element
    result := data;

    IF array_data IS NOT NULL AND jsonb_typeof(array_data) = 'array' THEN
        -- Find matching element index
        FOR idx IN 0..jsonb_array_length(array_data) - 1 LOOP
            element := array_data -> idx;
            IF element -> match_key = match_value THEN
                -- Found match, merge patch into this element
                result := jsonb_set(
                    result,
                    path || ARRAY[idx::text],
                    element || patch,
                    false
                );
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
CREATE OR REPLACE FUNCTION jsonb_ivm_available() RETURNS boolean
LANGUAGE sql IMMUTABLE
AS $$
    SELECT true; -- Always return true since we have stubs
$$;

COMMENT ON FUNCTION jsonb_smart_patch_nested IS 'Stub implementation for testing - merges patch at nested path';
COMMENT ON FUNCTION jsonb_smart_patch_array IS 'Stub implementation for testing - updates array element by match key';
COMMENT ON FUNCTION jsonb_smart_patch_scalar IS 'Stub implementation for testing - shallow merge';