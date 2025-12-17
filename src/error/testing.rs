#[cfg(any(test, feature = "pg_test"))]
/// # Panics
/// Panics if the result is `Ok` (operation succeeded when error was expected).
pub fn assert_error_sqlstate<T>(
    result: crate::TViewResult<T>,
    expected_sqlstate: &str,
) {
    match result {
        Err(e) => {
            assert_eq!(
                e.sqlstate(),
                expected_sqlstate,
                "Expected SQLSTATE {expected_sqlstate}, got {}: {e}",
                e.sqlstate()
            );
        }
        Ok(_) => {
            panic!("Expected error with SQLSTATE {expected_sqlstate}, but operation succeeded");
        }
    }
}

#[cfg(any(test, feature = "pg_test"))]
/// # Panics
/// Panics if the result is `Ok` (operation succeeded when error was expected).
pub fn assert_error_contains<T>(
    result: crate::TViewResult<T>,
    expected_substring: &str,
) {
    match result {
        Err(e) => {
            let message = e.to_string();
            assert!(
                message.contains(expected_substring),
                "Error message '{message}' does not contain '{expected_substring}'"
            );
        }
        Ok(_) => {
            panic!("Expected error containing '{expected_substring}', but operation succeeded");
        }
    }
}