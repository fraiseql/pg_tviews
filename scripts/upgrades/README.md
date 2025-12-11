# pg_tviews Upgrade Scripts

This directory contains automated scripts for upgrading pg_tviews between versions.

## Available Scripts

### upgrade-0.1.0-alpha-to-beta.1.sh

Upgrades from 0.1.0-alpha to 0.1.0-beta.1.

**Usage:**
```bash
./upgrade-0.1.0-alpha-to-beta.1.sh [database_name]
```

**What it does:**
- Checks current version
- Performs in-place upgrade via `ALTER EXTENSION`
- Falls back to full recreation if needed
- Verifies upgrade success
- Runs health check

**Requirements:**
- PostgreSQL running
- pg_tviews extension installed
- Database backup created
- Sufficient permissions

## Manual Upgrade Process

If automated scripts fail, follow these manual steps:

1. **Backup your database**
   ```bash
   pg_dump -Fc your_db > backup_$(date +%Y%m%d_%H%M%S).dump
   ```

2. **Stop your application**

3. **Upgrade extension**
   ```sql
   ALTER EXTENSION pg_tviews UPDATE;
   ```

4. **Verify upgrade**
   ```sql
   SELECT pg_tviews_version();
   SELECT * FROM pg_tviews_health_check();
   ```

5. **Restart application**

6. **Monitor for issues**

## Rollback

If upgrade fails:

```bash
# Restore from backup
pg_restore -d your_db backup_file.dump
```

## Version Compatibility

| From Version | To Version | Method | Risk Level |
|-------------|------------|--------|------------|
| 0.1.0-alpha | 0.1.0-beta.1 | In-place | Low |
| 0.1.0-beta.1 | 0.1.0-rc.1 | In-place | Medium |
| 0.1.0-rc.1 | 1.0.0 | Full recreation | High |

## Testing Upgrades

Always test upgrades in a staging environment first:

```bash
# Create test database
createdb pg_tviews_upgrade_test

# Restore production backup
pg_restore -d pg_tviews_upgrade_test production_backup.dump

# Run upgrade script
./upgrade-0.1.0-alpha-to-beta.1.sh pg_tviews_upgrade_test

# Test application functionality
# Drop test database when done
dropdb pg_tviews_upgrade_test
```

## Troubleshooting

### Extension won't update
- Check PostgreSQL logs
- Verify file permissions on extension files
- Ensure PostgreSQL can load the shared library

### TVIEWs not working after upgrade
- Check `pg_tview_meta` table exists
- Verify triggers are installed
- Recreate TVIEWs if metadata is corrupted

### Performance issues
- Run `ANALYZE` on TVIEW tables
- Check for missing indexes
- Review query plans

## See Also

- [Upgrade Guide](../docs/operations/upgrades.md) - Complete upgrade procedures
- [CHANGELOG.md](../../CHANGELOG.md) - Version change details