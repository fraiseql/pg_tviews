#!/usr/bin/env bash
# Every `extern "C-unwind"` function in src/ is called by PostgreSQL: a panic or
# an error longjmp must not cross it unguarded. Each must carry #[pg_guard]
# among the attributes right above it.
set -u
cd "$(dirname "$0")/.."
bad=$(awk '
  FNR == 1 { attrs = "" }
  /^[[:space:]]*#\[/ { attrs = attrs $0; next }
  /^[[:space:]]*\/\// { next }
  /extern "C-unwind" fn/ { if (attrs !~ /#\[pg_guard\]/) print FILENAME ":" FNR ": " $0 }
  { attrs = "" }
' $(find src -name '*.rs'))
if [[ -n "$bad" ]]; then
  echo "extern \"C-unwind\" functions without #[pg_guard]:"
  echo "$bad"
  exit 1
fi
echo "every extern \"C-unwind\" function carries #[pg_guard]"
