# shellcheck shell=bash
# The modes a differential harness runs each seed in. Source it.
#
# MODES is a space-separated list of modes; a mode is a comma-separated list of
# `switch:on|off`, each switch one of pg_tviews' diagnostic settings, which change
# how a refresh is done, never its result:
#   patch  pg_tviews.direct_patch_enabled  (off: every change recomputes rows from the view)
#   cache  pg_tviews.graph_cache_enabled and pg_tviews.table_cache_enabled
#          (off: every flush reads the catalog again)
# Default: "patch:on,cache:on", the shipped behaviour. The full matrix:
#   MODES="patch:on,cache:on patch:off,cache:on patch:on,cache:off"
# The settings are superuser-only: the harnesses connect as a superuser.

MODES="${MODES:-patch:on,cache:on}"
MODES_ALL="patch:on,cache:on patch:off,cache:on patch:on,cache:off"

# mode_options <mode>: the PGOPTIONS that select <mode>, or exit 2 on a bad mode.
mode_options() {
  local mode="$1" sw opts="" name value
  local IFS=,
  for sw in $mode; do
    name="${sw%%:*}" value="${sw#*:}"
    case "$value" in on | off) ;; *) echo "ERROR: bad mode switch '$sw' (on or off)" >&2; return 2 ;; esac
    case "$name" in
      patch) opts+=" -c pg_tviews.direct_patch_enabled=$value" ;;
      cache) opts+=" -c pg_tviews.graph_cache_enabled=$value -c pg_tviews.table_cache_enabled=$value" ;;
      *) echo "ERROR: bad mode switch '$sw' (patch or cache)" >&2; return 2 ;;
    esac
  done
  echo "${opts# }"
}

# check_modes: fail early (exit 2) on a bad MODES.
check_modes() {
  local m
  for m in $MODES; do mode_options "$m" >/dev/null || exit 2; done
}
