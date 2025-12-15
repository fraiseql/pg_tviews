#[cfg(feature = "pg_test")]
pub fn assert_error_sqlstate<T>(
    result: crate::TViewResult<T>,
    expected_sqlstate: &str,
) {
    match result {
        Err(e) => {
            assert_eq!(
                e.sqlstate(),
                expected_sqlstate,
                "Expected SQLSTATE {}, got {}: {}",
                expected_sqlstate,
                e.sqlstate(),
                e
            );
        }
        Ok(_) => {
            panic!("Expected error with SQLSTATE {}, but operation succeeded", expected_sqlstate);
        }
    }
}

#[cfg(feature = "pg_test")]
pub fn assert_error_contains<T>(
    result: crate::TViewResult<T>,
    expected_substring: &str,
) {
    match result {
        Err(e) => {
            let message = e.to_string();
            assert!(
                message.contains(expected_substring),
                "Error message '{}' does not contain '{}'",
                message,
                expected_substring
            );
        }
        Ok(_) => {
            panic!("Expected error containing '{}', but operation succeeded", expected_substring);
        }
    }
}