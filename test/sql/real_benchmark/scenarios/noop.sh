#!/usr/bin/env bash
# Physical scenario: refreshes that change nothing.
#
# Every step leaves the backing view's output identical, so an ideal refresh
# writes zero tv_ tuples. Today each queued key is rewritten (#72 evidence).
#
# Steps:
#   noop_self       UPDATE tb_post SET title = title over NOOP_ROWS rows (1 statement)
#   noop_repeat     the same statement 3 more times
#   noop_multi_path one transaction reaching the same POSTS_PER_USER tv_post keys
#                   three ways: the author cascade, a direct no-op update, and the
#                   direct no-op update again
#
# Usage:
#   ./noop.sh [--dry-run]
# Env: NOOP_ROWS (10000), USERS (500), POSTS_PER_USER (40), MODES, RUN_DIR

set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=SCRIPTDIR/../lib/common.sh
source "$here/../lib/common.sh"

NOOP_ROWS="${NOOP_ROWS:-10000}"
USERS="${USERS:-500}"
POSTS_PER_USER="${POSTS_PER_USER:-40}"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && { DRY_RUN=1; NOOP_ROWS=100; USERS=10; MODES="unlogged"; }
POSTS=$(( USERS * POSTS_PER_USER ))
(( NOOP_ROWS <= POSTS )) || { echo "NOOP_ROWS ($NOOP_ROWS) > posts ($POSTS)" >&2; exit 2; }
DB="bench_phys_noop"

setup_sql() {  # $1 = mode
  cat <<SQL
SET client_min_messages TO WARNING;
CREATE TABLE tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL
);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user (pk_user),
    title   text NOT NULL
);
CREATE INDEX ON tb_post (fk_user);
INSERT INTO tb_user (pk_user, name) SELECT g, 'user ' || g FROM generate_series(1, $USERS) g;
INSERT INTO tb_post (pk_post, fk_user, title)
SELECT g, 1 + (g - 1) % $USERS, 'post ' || g FROM generate_series(1, $POSTS) g;
SELECT pg_tviews_create('tv_user', \$v\$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user \$v\$, $(mode_options "$1"));
SELECT pg_tviews_create('tv_post', \$v\$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object('title', tb_post.title, 'author', v_user.data) AS data
    FROM tb_post LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user \$v\$, $(mode_options "$1"));
SQL
}

workload_sql() {
  local noop="UPDATE tb_post SET title = title WHERE pk_post <= $NOOP_ROWS;"
  phys_step_begin
  echo "$noop"
  phys_snap noop_self 1
  echo "$noop"; echo "$noop"; echo "$noop"
  phys_snap noop_repeat 3
  cat <<SQL
BEGIN;
UPDATE tb_user SET name = name WHERE pk_user = 1;
UPDATE tb_post SET title = title WHERE fk_user = 1;
UPDATE tb_post SET title = title WHERE fk_user = 1;
COMMIT;
SQL
  phys_snap noop_multi_path 1
  divergence_gate tv_post v_post pk_post
}

dry_run_check() {
  $PSQL -d "$1" -tA -c "
    SELECT CASE WHEN (SELECT count(*) FROM tv_post) = $POSTS
                 AND (SELECT count(*) FROM tv_post WHERE fk_user = 1) = $POSTS_PER_USER
                THEN 'DRY_OK' ELSE 'DRY_FAIL' END"
}

record_env
tmp="$(mktemp)"
for mode in $MODES; do
  echo "== noop ($mode): posts=$POSTS noop_rows=$NOOP_ROWS"
  db_fresh "$DB"
  bench_install "$DB"
  setup_sql "$mode" >"$tmp"
  run_script "$DB" "$tmp" "$RUN_DIR/noop_${mode}.setup.log" || exit 1
  if [[ $DRY_RUN == 1 ]]; then
    res="$(dry_run_check "$DB")"; echo "  $res"
    [[ "$res" == DRY_OK ]] || exit 1
    continue
  fi
  workload_sql >"$tmp"
  run_script "$DB" "$tmp" "$RUN_DIR/noop_${mode}.log" || exit 1
  phys_dump "$DB" noop "$mode" || exit 1
  echo "UPDATE tb_post SET title = title WHERE pk_post <= $NOOP_ROWS;" >"$tmp"
  explain_pass "$DB" "$tmp" "noop_${mode}" 'tv_post'
done
rm -f "$tmp"
db_exec "DROP DATABASE IF EXISTS $DB"
echo "noop done -> $RUN_DIR"
