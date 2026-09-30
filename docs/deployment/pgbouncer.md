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

`PREPARE TRANSACTION` flushes the refresh queue first, as `COMMIT` does, so the TVIEW writes belong to the prepared transaction: `COMMIT PREPARED` applies them and `ROLLBACK PREPARED` discards them. No pg_tviews-specific command is involved.

## Known Limitations

None - all features work correctly through PgBouncer.