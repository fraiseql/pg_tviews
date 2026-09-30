# Extension upgrade scripts

This directory holds only upgrade scripts, `pg_tviews--<from>--<to>.sql`, one per
pair of consecutive releases. `cargo pgrx install` / `package` copies them next to
the generated install script; `ALTER EXTENSION pg_tviews UPDATE` chains them.

The rules are in [docs/development/extension-versioning.md](../docs/development/extension-versioning.md):
a released script is never edited, every PR that changes the extension SQL adds its
statements to the pending script, and a script that changes what registration
derives ends with `UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;`.
