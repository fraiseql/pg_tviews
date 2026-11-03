#!/usr/bin/env bash
# Every PL/pgSQL handler of all errors in the extension SQL keeps what it catches:
# within the handler it re-raises (`RAISE;`, `RAISE EXCEPTION`, `ERRCODE = SQLSTATE`),
# or the line above `EXCEPTION WHEN OTHERS` is a comment saying why it may swallow
# every error (`-- catches every error: <why>`). A handler that turns a deadlock or a
# lock timeout into a WARNING or `false` loses its SQLSTATE and commits half-done work.
set -u
cd "$(dirname "$0")/.."
bad=""
while IFS=: read -r file line _; do
  above=$(sed -n "$((line - 1))p" "$file")
  if grep -qiE -- '--[[:space:]]*catches every error:' <<<"$above"; then
    continue
  fi
  handler=$(sed -n "$((line + 1)),$((line + 8))p" "$file" | sed '/^[[:space:]]*END[;[:space:]]*$/q')
  if grep -qE "RAISE;|RAISE EXCEPTION|ERRCODE = SQLSTATE" <<<"$handler"; then
    continue
  fi
  bad+="$file:$line"$'\n'
done < <(grep -rnE 'WHEN OTHERS THEN' src sql/install --include='*.rs' --include='*.sql')
if [[ -n "$bad" ]]; then
  echo "PL/pgSQL handlers that swallow every error (re-raise, or say why above the EXCEPTION line):"
  printf '%s' "$bad"
  exit 1
fi
echo "every PL/pgSQL handler of all errors re-raises or says why it does not"
