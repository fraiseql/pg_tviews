#!/usr/bin/env bash
# swap.sh <package root>: install a build packaged with `cargo pgrx package
# --out-dir <package root>` into the dev server's installation and restart it.
# The old library is removed first: overwriting the mapped .so in place crashes
# the running rebuild launcher. Prints the installed library's md5.
set -euo pipefail
INSTALL="${PGRX_INSTALL:-$HOME/.pgrx/18.1/pgrx-install}"
DATA="${PGDATA:-$HOME/.pgrx/data-18}"
pkg="${1:?usage: swap.sh <package root>}"
"$INSTALL/bin/pg_ctl" -D "$DATA" stop -m fast -w -t 20 >/dev/null 2>&1 ||
  "$INSTALL/bin/pg_ctl" -D "$DATA" stop -m immediate -w >/dev/null
rm -f "$INSTALL/lib/postgresql/pg_tviews.so"
cp -r "$pkg$INSTALL/." "$INSTALL/"
"$INSTALL/bin/pg_ctl" -D "$DATA" start -l "$DATA.log" -w >/dev/null
md5sum "$INSTALL/lib/postgresql/pg_tviews.so"
