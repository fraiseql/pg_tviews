#!/usr/bin/env bash
# Refuse to publish a private finding before the release that fixes it.
#
# A security finding stays private until its fix is released (SECURITY.md).
# Commits and files cite it by ID only. This check greps what a push would make
# public for the identifiers, setting names and phrases of each private
# finding's mechanism, listed in an untracked file of the private process, and
# fails on a match:
#   - the tracked tree of the commit being pushed, file by file;
#   - each commit to push: its message and the lines its diff adds.
#
# Patterns file (default .phases/security-beta36/disclosure-patterns.txt,
# override with DISCLOSURE_PATTERNS): one rule per line, `ID<TAB>regex`, where
# regex is an extended, case-insensitive regex; `re1 && re2` matches when every
# part matches in the same file or the same commit. Lines starting with # and
# blank lines are ignored.
#
# Usage:
#   scripts/check-disclosure.sh                 HEAD's tree + commits not on any remote
#   scripts/check-disclosure.sh FROM TO         TO's tree + commits FROM..TO
# As a pre-push hook (.pre-commit-config.yaml, stage pre-push), the range comes
# from PRE_COMMIT_FROM_REF / PRE_COMMIT_TO_REF.
#
# Without the patterns file the check cannot run and fails, unless
# DISCLOSURE_OPTIONAL=1 (a clone outside the private process).
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
patterns=${DISCLOSURE_PATTERNS:-.phases/security-beta36/disclosure-patterns.txt}

if [[ ! -f $patterns ]]; then
    if [[ ${DISCLOSURE_OPTIONAL:-0} == 1 ]]; then
        echo "check-disclosure: no patterns file ($patterns), skipped (DISCLOSURE_OPTIONAL=1)"
        exit 0
    fi
    echo "check-disclosure: patterns file $patterns not found" >&2
    echo "  (set DISCLOSURE_PATTERNS, or DISCLOSURE_OPTIONAL=1 outside the private process)" >&2
    exit 2
fi

zero=0000000000000000000000000000000000000000
from=${1:-${PRE_COMMIT_FROM_REF:-}}
to=${2:-${PRE_COMMIT_TO_REF:-HEAD}}
to=$(git rev-parse --verify "$to^{commit}")
if [[ -z $from || $from == "$zero" ]]; then
    range=("$to" --not --remotes)
else
    range=("$(git rev-parse --verify "$from^{commit}")..$to")
fi

ids=()
rules=()
while IFS= read -r line || [[ -n $line ]]; do
    [[ -z ${line//[[:space:]]/} || $line == \#* ]] && continue
    if [[ $line != *$'\t'* ]]; then
        echo "check-disclosure: malformed rule (no tab): $line" >&2
        exit 2
    fi
    ids+=("${line%%$'\t'*}")
    rules+=("${line#*$'\t'}")
done <"$patterns"
if ((${#rules[@]} == 0)); then
    echo "check-disclosure: $patterns holds no rule" >&2
    exit 2
fi

# Split a rule into its parts on ' && '.
parts_of() {
    local rule=$1
    parts=()
    while [[ $rule == *' && '* ]]; do
        parts+=("${rule%% && *}")
        rule=${rule#* && }
    done
    parts+=("$rule")
}

failed=0
report() {
    echo "check-disclosure: $1 matches the private patterns of $2" >&2
    failed=1
}

# The tree: per rule, the files matching every part.
for i in "${!rules[@]}"; do
    parts_of "${rules[$i]}"
    files=$(git grep -l -I -i -E -e "${parts[0]}" "$to" -- . | sed "s/^$to://" || true)
    for p in "${parts[@]:1}"; do
        [[ -z $files ]] && break
        files=$(printf '%s\n' "$files" | while IFS= read -r f; do
            git show "$to:$f" | grep -q -i -E -e "$p" && printf '%s\n' "$f"
        done || true)
    done
    while IFS= read -r f; do
        [[ -n $f ]] && report "file $f" "${ids[$i]}"
    done <<<"$files"
done

# The commits: message and added lines, per commit.
while IFS= read -r c; do
    [[ -z $c ]] && continue
    text=$(git log -1 --format=%B "$c"; git show --format= --no-color --unified=0 "$c" |
        grep -E '^\+' | grep -v -E '^\+\+\+ ' || true)
    for i in "${!rules[@]}"; do
        parts_of "${rules[$i]}"
        hit=1
        for p in "${parts[@]}"; do
            grep -q -i -E -e "$p" <<<"$text" || { hit=0; break; }
        done
        ((hit)) && report "commit $(git rev-parse --short "$c")" "${ids[$i]}"
    done
done < <(git rev-list "${range[@]}")

if ((failed)); then
    echo "check-disclosure: refused. Cite the finding by ID only until its fix is released." >&2
    exit 1
fi
echo "check-disclosure: clean ($(git rev-list --count "${range[@]}") commit(s), tree of $(git rev-parse --short "$to"))"
