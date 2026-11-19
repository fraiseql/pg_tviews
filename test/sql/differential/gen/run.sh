#!/usr/bin/env bash
# Generated shapes: per seed, gen.py writes random schemas in the project's
# convention and TVIEW definitions drawn from its grammar (joins, embedded TVIEWs,
# subquery aggregates, jsonb_agg arrays, UNION ALL, CTEs, GROUP BY, DISTINCT ON,
# soft-delete filters), then random writes: single- and multi-row, rows moving
# between parents, key changes, transactions, and mutation functions writing in a
# few statements and reading a TVIEW back. Every TVIEW must equal its view after
# each top-level statement, and inside each mutation after its writes.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/sql/differential/gen/run.sh [SHAPES] [SEEDS] [STATEMENTS]
#
# SHAPES TVIEW definitions per seed (default 20), split into cases of 3 to 6, each
# case its own database; SEEDS a space-separated list (default "1 2 3"); STATEMENTS
# random writes per case (default 40). A definition is created, or refused with a
# documented SQLSTATE (REFUSED, listed in lib.sql); any other error, a divergence or
# a stale read fails the case, which prints its seed and keeps its SQL. Before the
# writes, each TVIEW's stored read sets are checked by perturbation (gen_perturb in
# lib.sql): an UPDATE of a column outside them must leave its view's rows alone.
#
# MODES (test/lib/modes.sh) runs every case once per mode, as run.sh does.
# GEN_PGOPTIONS adds settings to every session (mutants/run.sh names its mutant).
# GEN_FEATURES (comma-separated) adds the opt-in kinds and value forms of gen.py's
# OPT_IN (wholerow, syscol, unordered, exact), each left out while its defect is open.
# GEN_SELFTEST=1 writes with session_replication_role = replica, so no pg_tviews
# trigger fires: every case must then fail, which proves the oracle sees a refresh
# that does not happen. GEN_SELFTEST=2 lets the oracle read in the session's
# settings (another TimeZone) instead of the owner frame's: some case must then
# diverge, which proves the oracle reads in the frame. GEN_SELFTEST=3 runs every
# case once per wrong row security pattern (lib.sql: a tv_* without row security,
# a child table reader A reads less of than its parent, a tv_* granted to a reader
# who may not select a column it copies): for each, some case must leak, which
# proves the visibility oracle (../visibility.sql) sees it.
#
# Roles: every TVIEW is created by a role that is no superuser and owns no base
# table, every write is made by another (some tables hide rows from it by row
# security the owner bypasses), and the oracle reads as the owner (lib.sql). Four
# readers read through the tenant policies: the visibility oracle evaluates each
# definition as each of them.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../../lib/modes.sh
source "$here/../../../lib/modes.sh"
check_modes
shapes="${1:-20}"
seeds="${2:-1 2 3}"
statements="${3:-40}"
selftest="${GEN_SELFTEST:-0}"
# Opt-in writes of gen.py (OPT_IN), each left out while its defect is open.
features="${GEN_FEATURES-}"
db="pg_tviews_gen_$$"
# The roles harness (lib.sql): the TVIEWs' owner and the writer, for this run.
owner="gen_owner_$$"
writer="gen_writer_$$"
readers=(a b c n)
work="$(mktemp -d)"
# Failing cases are copied to GEN_KEEP (a new temporary directory by default).
keep="${GEN_KEEP-}"
trap 'rm -rf "$work"; psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1;
      for r in $owner $writer gen_reader_{a,b,c,n}_$$; do psql -d postgres -qc "DROP ROLE IF EXISTS $r"; done >/dev/null 2>&1' EXIT
psql -d postgres -qc "CREATE ROLE $owner NOLOGIN BYPASSRLS" -c "CREATE ROLE $writer NOLOGIN" >/dev/null \
  || { echo "ERROR: cannot create the roles $owner and $writer"; exit 2; }
reader_vars=()
for r in "${readers[@]}"; do
  psql -d postgres -qc "CREATE ROLE gen_reader_${r}_$$ NOLOGIN" >/dev/null \
    || { echo "ERROR: cannot create the role gen_reader_${r}_$$"; exit 2; }
  reader_vars+=(-v "gen_reader_$r=gen_reader_${r}_$$")
done
# The row security patterns each case runs under: the documented one, or under
# self-test 3 each wrong one (lib.sql), each of which some case must leak.
wrongs=(none)
[[ "$selftest" == 3 ]] && wrongs=(nopolicy narrower revoked)
declare -A leaked=()
frame_selftest=off
[[ "$selftest" == 2 ]] && frame_selftest=on
seen=0

failed=0
cases=0
for seed in $seeds; do
  mkdir -p "$work/$seed"
  python3 -I "$here/gen.py" "$seed" "$shapes" "$statements" "$work/$seed" "$features" >"$work/$seed/index" \
    || { echo "ERROR: gen.py failed for seed $seed"; exit 2; }
  while read -r file n; do
    for wrong in "${wrongs[@]}"; do
    for mode in $MODES; do
      PGOPTIONS="$(mode_options "$mode")"
      [[ "$selftest" == 1 ]] && PGOPTIONS+=" -c session_replication_role=replica"
      # Settings of the caller's (the mutants runner names its mutant here).
      PGOPTIONS+="${GEN_PGOPTIONS:+ $GEN_PGOPTIONS}"
      # A zone the owner frame overrides, so an oracle reading outside it diverges
      # (PGTZ, which libpq sends, wins over a TimeZone in PGOPTIONS).
      [[ "$selftest" == 2 ]] && export PGTZ=America/New_York
      export PGOPTIONS
      cases=$((cases+1))
      psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
      psql -d postgres -qc "CREATE DATABASE $db" >/dev/null || { echo "ERROR: cannot create $db"; exit 2; }
      psql -d postgres -qc "ALTER DATABASE $db SET search_path = \"\$user\", public, tviews" >/dev/null
      out="$work/out"
      psql -d "$db" -X -q -v ON_ERROR_STOP=1 -v gen_owner="$owner" -v gen_writer="$writer" \
        "${reader_vars[@]}" -v gen_wrong="$wrong" \
        -v gen_selftest_frame="$frame_selftest" -f "$here/lib.sql" -f "$work/$seed/$file" >"$out" 2>&1
      rc=$?
      refused="$(grep -c 'REFUSED' "$out")"
      name="seed $seed ${file%.sql} [$mode] ($n shapes, $statements statements, $refused refused)"
      if [[ "$selftest" == 3 ]]; then
        name+=" [$wrong]"
        if [[ $rc -ne 0 ]] && grep -q 'LEAKED' "$out"; then
          leaked[$wrong]=1
          echo "PASS  $name: the oracle saw the leak"
        elif [[ $rc -ne 0 ]]; then
          echo "FAIL  $name: an error other than a leak"
          grep -E -A3 'ERROR' "$out" | head -6 | sed 's/^/      /'
          failed=$((failed+1))
        else
          echo "PASS  $name: nothing it reads leaks under this pattern"
        fi
      elif [[ "$selftest" == 2 ]]; then
        # Only a case rendering a value the frame pins (a timestamptz) can diverge.
        if [[ $rc -ne 0 ]] && grep -qE 'DIVERGED|STALE' "$out"; then
          seen=$((seen+1))
          echo "PASS  $name: the oracle saw the settings outside the frame"
        elif [[ $rc -ne 0 ]]; then
          echo "FAIL  $name: an error other than a divergence"
          grep -E -A3 'ERROR' "$out" | head -6 | sed 's/^/      /'
          failed=$((failed+1))
        else
          echo "PASS  $name: nothing it renders depends on those settings"
        fi
      elif [[ "$selftest" == 1 ]]; then
        if [[ $rc -ne 0 ]] && grep -qE 'DIVERGED|STALE' "$out"; then
          echo "PASS  $name: the oracle saw the missing refresh"
        else
          echo "FAIL  $name: no divergence without triggers"
          failed=$((failed+1))
        fi
      elif [[ $rc -eq 0 ]]; then
        echo "PASS  $name"
      else
        failed=$((failed+1))
        [[ -n "$keep" ]] || keep="$(mktemp -d -t pg_tviews_gen_failed.XXXXXX)"
        mkdir -p "$keep"
        cp "$work/$seed/$file" "$keep/seed$seed-$file"
        echo "FAIL  $name"
        grep -E -A6 'ERROR' "$out" | head -20 | sed 's/^/      /'
        echo "      replay: psql -f $here/lib.sql -f $keep/seed$seed-$file"
        echo "      regenerate: python3 -I $here/gen.py $seed $shapes $statements <dir> $features"
      fi
      grep -E 'REFUSED|SKIPPED' "$out" | sed 's/^.*WARNING: */      /'
    done
    done
  done <"$work/$seed/index"
done
unset PGOPTIONS
if [[ "$selftest" == 3 ]]; then
  for wrong in "${wrongs[@]}"; do
    if [[ -z "${leaked[$wrong]-}" ]]; then
      echo "FAIL  no case leaked under the wrong pattern $wrong"
      failed=$((failed+1))
    fi
  done
fi
if [[ "$selftest" == 2 && $seen -eq 0 ]]; then
  echo "FAIL  no case diverged with the oracle outside the owner frame"
  failed=$((failed+1))
fi
echo "$((cases-failed))/$cases cases passed"
[[ "$failed" -eq 0 ]]
