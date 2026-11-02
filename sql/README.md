# Extension SQL

- `install/*.sql`: the install script's parts, included by `src/install_sql.rs` with
  `extension_sql_file!` in the order their `requires` give.
- `pg_tviews--<previous>--<current>.sql`: the one update script, from the release
  before this one. An older install is dropped and recreated
  ([docs/operations/upgrade.md](../docs/operations/upgrade.md)). `cargo pgrx install` /
  `package` copies it next to the generated install script.

The rules are in [docs/development/extension-versioning.md](../docs/development/extension-versioning.md):
a released script is never edited, every PR that changes the extension SQL adds its
statements to the pending script, and the script ends by re-deriving every TVIEW
(`pg_tviews_reregister_all(strict => true)`), so an update leaves no TVIEW on an
older plan.
