# Manual Test Files

This directory contains manual test files for debugging and verification.

## Hook Tests

- `test_hook_verification.sql` - Comprehensive test of CREATE/DROP hooks
- `test_hook_simple.sql` - Simple hook behavior test

## Feature Tests

- `test_task2_single_row_refresh.sql` - Single row refresh functionality
- `test_task3_cascade.sql` - Cascade refresh testing

## Usage

Run these tests manually with:
```bash
psql -h localhost -p 28817 -d postgres -f test/manual/<test_file>.sql
```
