#!/usr/bin/env bash
# 1. Every `extern "C-unwind"` function in src/ is called by PostgreSQL: a panic or
#    an error longjmp must not cross it unguarded. Each must carry #[pg_guard]
#    among the attributes right above it.
# 2. Every backend function src/ declares itself (an `extern "C" {` block, for a
#    symbol the pgrx bindings lack) is private, named raw_*, and called only inside
#    pg_sys::ffi::pg_guard_ffi_boundary, on its line or the line after it: an ERROR it raises must
#    unwind pg_tviews' frames, not longjmp over them (E1-13).
#
# Usage: scripts/check-ffi-guards.sh [file ...]   (default: every src/**/*.rs)
set -u
cd "$(dirname "$0")/.."
if [[ $# -gt 0 ]]; then files=("$@"); else mapfile -t files < <(find src -name '*.rs'); fi
bad=$(awk '
  FNR == 1 { attrs = "" }
  /^[[:space:]]*#\[/ { attrs = attrs $0; next }
  /^[[:space:]]*\/\// { next }
  /extern "C-unwind" fn/ { if (attrs !~ /#\[pg_guard\]/) print FILENAME ":" FNR ": " $0 }
  { attrs = "" }
' "${files[@]}")
imports=$(python3 -I - "${files[@]}" <<'PY'
import re, sys
bad = []
for path in sys.argv[1:]:
    lines = open(path, encoding="utf-8").read().splitlines()
    names, inside = set(), False
    for no, line in enumerate(lines, 1):
        code = line.split("//")[0]
        if re.search(r'\bextern "C" \{', code):
            inside = True
            continue
        if inside:
            if code.strip() == "}":
                inside = False
                continue
            m = re.search(r'\bfn\s+(\w+)', code)
            if m:
                if re.search(r'\bpub\b', code):
                    bad.append(f"{path}:{no}: an imported backend function is public: {line.strip()}")
                if not m.group(1).startswith("raw_"):
                    bad.append(f"{path}:{no}: an imported backend function is not named raw_*: {line.strip()}")
                names.add(m.group(1))
    if not names:
        continue
    inside = False
    for no, line in enumerate(lines, 1):
        code = line.split("//")[0]
        if re.search(r'\bextern "C" \{', code):
            inside = True
        if inside:
            inside = code.strip() != "}"
            continue
        for name in names:
            guarded = "pg_guard_ffi_boundary" in code or (
                no > 1 and "pg_guard_ffi_boundary(|| {" in lines[no - 2])
            if re.search(rf'\b{name}\s*\(', code) and not guarded:
                bad.append(f"{path}:{no}: {name} called outside pg_guard_ffi_boundary: {line.strip()}")
for b in bad:
    print(b)
PY
)
rc=0
if [[ -n "$bad" ]]; then
  echo "extern \"C-unwind\" functions without #[pg_guard]:"
  echo "$bad"
  rc=1
fi
if [[ -n "$imports" ]]; then
  echo "backend functions declared in src/ and called unguarded:"
  echo "$imports"
  rc=1
fi
[[ $rc == 0 ]] || exit 1
echo "every extern \"C-unwind\" function carries #[pg_guard]; every imported backend function is called through pg_guard_ffi_boundary"
