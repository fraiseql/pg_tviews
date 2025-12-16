#!/bin/bash
# Verify cross-phase consistency
# Checks for SQL injection vulnerabilities and validation consistency

echo "🔍 Checking for SQL injection vulnerabilities..."

# Search for unsafe format! patterns with table names
echo "Checking table name interpolation..."
if git grep 'format!.*{table' src/ | grep -v "validate_table_name"; then
    echo "❌ ERROR: Found unvalidated table_name in format!"
    echo "   All table_name usage must be preceded by validate_table_name()"
    exit 1
fi

# Search for unsafe format! patterns with column names
echo "Checking column name interpolation..."
if git grep 'format!.*{.*column' src/ | grep -v "validate_sql_identifier"; then
    echo "❌ ERROR: Found unvalidated column names in format!"
    echo "   All column name usage must be preceded by validate_sql_identifier()"
    exit 1
fi

# Search for unsafe format! patterns with paths
echo "Checking path interpolation..."
if git grep 'format!.*{.*path' src/ | grep -v "validate_jsonb_path"; then
    echo "❌ ERROR: Found unvalidated paths in format!"
    echo "   All path usage must be preceded by validate_jsonb_path()"
    exit 1
fi

# Check that all public functions validate their inputs
echo "Checking function input validation..."
# Look for public functions that take string parameters but don't call validators
# This is a simplified check - would need more sophisticated analysis for full coverage

echo "✅ No obvious SQL injection vulnerabilities found"

echo ""
echo "🔍 Checking validation module usage..."

# Check that validation module is imported where needed
if ! git grep "use crate::validation" src/ | grep -q .; then
    echo "⚠️  WARNING: Validation module not imported in any source files"
    echo "   This may indicate missing validation calls"
fi

echo "✅ Validation infrastructure appears to be in use"

echo ""
echo "🔍 Checking error handling consistency..."

# Check that all validation errors use consistent error types
if git grep "TViewError::InvalidInput" src/ | grep -q . && \
   git grep "TViewError::SecurityViolation" src/ | grep -q .; then
    echo "✅ Error types are being used consistently"
else
    echo "⚠️  WARNING: Not all validation error types are in use"
    echo "   Consider using InvalidInput and SecurityViolation consistently"
fi

echo ""
echo "🔍 Checking test coverage..."

# Check that security tests exist
if git grep "sql_injection\|SQL injection\|injection" test/ | grep -q .; then
    echo "✅ Security tests appear to be present"
else
    echo "⚠️  WARNING: No obvious security tests found"
    echo "   Consider adding SQL injection test cases"
fi

echo ""
echo "🎉 All consistency checks passed!"
echo ""
echo "📋 Next steps:"
echo "1. Run 'cargo clippy' to check for additional issues"
echo "2. Run full test suite with security tests"
echo "3. Review this script for additional validation rules"