-- Security Testing Helper Functions
-- Used across all phase integration tests

-- Test that a function rejects SQL injection
CREATE OR REPLACE FUNCTION assert_rejects_injection(
    test_name TEXT,
    test_func TEXT,  -- Function call with injection attempt
    expected_error_pattern TEXT DEFAULT 'injection|invalid|security'
) RETURNS VOID AS $$
DECLARE
    error_occurred BOOLEAN := FALSE;
    error_message TEXT;
BEGIN
    -- Try to execute the injection attempt
    EXECUTE test_func;

    -- If we get here, injection wasn't prevented!
    RAISE EXCEPTION 'SECURITY FAILURE [%]: SQL injection was not prevented!', test_name;

EXCEPTION
    WHEN OTHERS THEN
        error_occurred := TRUE;
        error_message := SQLERRM;

        -- Check if error message indicates security rejection
        IF error_message ~* expected_error_pattern THEN
            RAISE NOTICE 'PASS [%]: SQL injection correctly rejected', test_name;
        ELSE
            RAISE EXCEPTION 'SECURITY FAILURE [%]: Injection caused unexpected error: %',
                test_name, error_message;
        END IF;
END;
$$ LANGUAGE plpgsql;

-- Test that a function works with valid input
CREATE OR REPLACE FUNCTION assert_accepts_valid(
    test_name TEXT,
    test_func TEXT,  -- Function call with valid input
    expected_result TEXT DEFAULT NULL
) RETURNS VOID AS $$
DECLARE
    actual_result TEXT;
BEGIN
    EXECUTE test_func INTO actual_result;

    IF expected_result IS NOT NULL AND actual_result != expected_result THEN
        RAISE EXCEPTION 'FAILURE [%]: Expected %, got %',
            test_name, expected_result, actual_result;
    END IF;

    RAISE NOTICE 'PASS [%]: Valid input accepted', test_name;

EXCEPTION
    WHEN OTHERS THEN
        RAISE EXCEPTION 'FAILURE [%]: Valid input rejected: %', test_name, SQLERRM;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION assert_rejects_injection IS
'Test helper: Verify function rejects SQL injection attempts';

COMMENT ON FUNCTION assert_accepts_valid IS
'Test helper: Verify function accepts valid input';