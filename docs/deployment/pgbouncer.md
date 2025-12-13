# PgBouncer Compatibility

## Supported Modes

pg_tviews is compatible with all PgBouncer pooling modes:

- **Transaction pooling**: ✅ Fully supported (recommended)
- **Session pooling**: ✅ Fully supported
- **Statement pooling**: ⚠️ Not recommended (TVIEW state is per-transaction)

## Configuration

### Transaction Pooling (Recommended)

```ini
pool_mode = transaction
```

Queue is automatically cleared via `DISCARD ALL` between transactions.

### Two-Phase Commit (2PC)

2PC is fully supported. Queue entries are persisted in `pg_tview_pending_refreshes` during `PREPARE TRANSACTION` and restored on `COMMIT PREPARED`.

## Known Limitations

None - all features work correctly through PgBouncer.