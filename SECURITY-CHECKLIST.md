# Security Audit Checklist

## Code Review

- [ ] No `format!()` with unvalidated user input
- [ ] All SQL uses parameterized queries where possible
- [ ] All identifiers validated before interpolation
- [ ] All paths validated for syntax and injection
- [ ] No `unwrap()` on user input
- [ ] All error messages sanitize sensitive data
- [ ] No secrets in debug/log output

## Testing

- [ ] SQL injection tests for each function
- [ ] Malformed input tests
- [ ] Boundary value tests (empty, max length, special chars)
- [ ] Fallback tests (without jsonb_ivm)
- [ ] Integration tests with malicious metadata
- [ ] DoS tests (large inputs, deep recursion)

## Documentation

- [ ] Security constraints documented
- [ ] Valid input examples provided
- [ ] Invalid input examples provided
- [ ] Error messages guide users to fix issues
- [ ] Installation security notes included

## Deployment

- [ ] Release notes mention security fixes
- [ ] Migration guide includes validation updates
- [ ] Breaking changes clearly documented
- [ ] Security advisory if upgrading existing code