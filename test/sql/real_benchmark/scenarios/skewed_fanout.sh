#!/usr/bin/env bash
# Physical scenario: skewed-fan-out cascade (user -> post -> comment).
#
# tv_post embeds its author (v_user.data); tv_comment embeds its post
# (v_post.data, which carries the author), so a user change cascades two hops.
# Post ownership is heavily skewed: three "celebrity" users own CELEB posts,
# the rest follow a power-law tail. Comments are skewed over posts independently
# of the author.
#
# Steps: user_p50 (5 median users), user_p99 (3 users at p99), post_title
# (20 posts), then one step per celebrity. Fan-out p50/p95/p99 per edge is
# written to $RUN_DIR/fanout.csv.
#
# Usage:
#   ./skewed_fanout.sh [--dry-run]
# Env: SCALE (1 -> 10k users, 300k posts, 700k comments, celebrities
#      100k/30k/10k posts; SCALE=10 for 10M rows), or USERS/POSTS/COMMENTS/
#      CELEB ("c1 c2 c3") individually; MODES, RUN_DIR

set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=SCRIPTDIR/../lib/common.sh
source "$here/../lib/common.sh"

SCALE="${SCALE:-1}"
USERS="${USERS:-$(( 10000 * SCALE ))}"
POSTS="${POSTS:-$(( 300000 * SCALE ))}"
COMMENTS="${COMMENTS:-$(( 700000 * SCALE ))}"
CELEB="${CELEB:-$(( 100000 * SCALE )) $(( 30000 * SCALE )) $(( 10000 * SCALE ))}"
DRY_RUN=0
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY_RUN=1 USERS=100 POSTS=3000 COMMENTS=5000 CELEB="1000 300 100" MODES="unlogged"
fi
read -r C1 C2 C3 <<<"$CELEB"
TEMPLATE="bench_phys_fanout_data"
DB="bench_phys_fanout"

base_sql() {
  cat <<SQL
SET client_min_messages TO WARNING;
SELECT setseed(0.42);
CREATE TABLE tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    bio     text
);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user (pk_user),
    title   text NOT NULL
);
CREATE TABLE tb_comment (
    pk_comment int PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post    int NOT NULL REFERENCES tb_post (pk_post),
    body       text NOT NULL
);
CREATE INDEX ON tb_post (fk_user);
CREATE INDEX ON tb_comment (fk_post);

INSERT INTO tb_user (pk_user, name, bio)
SELECT g, 'user ' || g, 'bio of user ' || g FROM generate_series(1, $USERS) g;

-- Celebrities 1..3 first, then a power-law tail over users 4..N; pk order is
-- shuffled so post pk carries no information about the author.
INSERT INTO tb_post (pk_post, fk_user, title)
SELECT row_number() OVER (ORDER BY random()), fk_user, 'post title'
FROM (
    SELECT 1 AS fk_user FROM generate_series(1, $C1)
    UNION ALL SELECT 2 FROM generate_series(1, $C2)
    UNION ALL SELECT 3 FROM generate_series(1, $C3)
    UNION ALL SELECT 4 + floor(($USERS - 3) * random() ^ 2)::int
              FROM generate_series(1, $POSTS - $C1 - $C2 - $C3)
) s;

INSERT INTO tb_comment (pk_comment, fk_post, body)
SELECT g, 1 + floor($POSTS * random() ^ 2)::int, 'comment body ' || g
FROM generate_series(1, $COMMENTS) g;
ANALYZE;
SQL
}

tview_sql() {  # $1 = mode
  cat <<SQL
SET client_min_messages TO WARNING;
$(mode_sql "$1")
SELECT pg_tviews_create('tv_user', \$v\$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data
    FROM tb_user \$v\$);
SELECT pg_tviews_create('tv_post', \$v\$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object('title', tb_post.title, 'author', v_user.data) AS data
    FROM tb_post LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user \$v\$);
SELECT pg_tviews_create('tv_comment', \$v\$
    SELECT tb_comment.pk_comment, tb_comment.id, tb_comment.fk_post,
           jsonb_build_object('body', tb_comment.body, 'post', v_post.data) AS data
    FROM tb_comment LEFT JOIN v_post ON v_post.pk_post = tb_comment.fk_post \$v\$);
SQL
}

fanout_csv() {  # $1 = db; per-edge distribution of dependents per parent key
  local out="$RUN_DIR/fanout.csv"
  [[ -s "$out" ]] || echo "scenario,edge,parents,p50,p95,p99,max,mean" >"$out"
  $PSQL -d "$1" -tA -F, -c "
    WITH up AS (SELECT u.pk_user, count(p.pk_post) AS n
                FROM tb_user u LEFT JOIN tb_post p ON p.fk_user = u.pk_user GROUP BY 1),
         pc AS (SELECT p.pk_post, p.fk_user, count(c.pk_comment) AS n
                FROM tb_post p LEFT JOIN tb_comment c ON c.fk_post = p.pk_post GROUP BY 1, 2),
         uc AS (SELECT fk_user, sum(n) AS n FROM pc GROUP BY 1),
         edges AS (SELECT 'user->post' AS edge, n FROM up
                   UNION ALL SELECT 'post->comment', n FROM pc
                   UNION ALL SELECT 'user->comment', n FROM uc)
    SELECT 'skewed_fanout', edge, count(*),
           percentile_disc(0.5) WITHIN GROUP (ORDER BY n),
           percentile_disc(0.95) WITHIN GROUP (ORDER BY n),
           percentile_disc(0.99) WITHIN GROUP (ORDER BY n),
           max(n), round(avg(n), 2)
    FROM edges GROUP BY edge ORDER BY edge" >>"$out"
}

# Space-separated pk_user values whose post count sits at percentile $2 (tail only).
users_at() {  # $1 = db, $2 = fraction, $3 = how many
  $PSQL -d "$1" -tA -c "
    WITH c AS (SELECT fk_user, count(*) AS n FROM tb_post WHERE fk_user > 3 GROUP BY 1),
         t AS (SELECT percentile_disc($2) WITHIN GROUP (ORDER BY n) AS n FROM c)
    SELECT string_agg(fk_user::text, ' ')
    FROM (SELECT fk_user FROM c, t WHERE c.n = t.n ORDER BY fk_user LIMIT $3) s"
}

workload_sql() {  # $1 = db
  local u i
  phys_step_begin
  for u in $(users_at "$1" 0.5 5); do
    echo "UPDATE tb_user SET name = name || '.' WHERE pk_user = $u;"
  done
  phys_snap user_p50 5
  for u in $(users_at "$1" 0.99 3); do
    echo "UPDATE tb_user SET name = name || '.' WHERE pk_user = $u;"
  done
  phys_snap user_p99 3
  for ((i = 1; i <= 20; i++)); do
    echo "UPDATE tb_post SET title = title || '.' WHERE pk_post = $(( 1 + (i * 7919) % POSTS ));"
  done
  phys_snap post_title 20
  for u in 3 2 1; do
    echo "UPDATE tb_user SET name = name || '.' WHERE pk_user = $u;"
    phys_snap "celebrity_$(case $u in 1) echo "$C1" ;; 2) echo "$C2" ;; 3) echo "$C3" ;; esac)" 1
  done
  divergence_gate tv_user v_user pk_user
  divergence_gate tv_post v_post pk_post
  divergence_gate tv_comment v_comment pk_comment
}

dry_run_check() {  # $1 = db
  $PSQL -d "$1" -tA -c "
    SELECT CASE WHEN (SELECT count(*) FROM tv_user) = $USERS
                 AND (SELECT count(*) FROM tv_post) = $POSTS
                 AND (SELECT count(*) FROM tv_comment) = $COMMENTS
                 AND (SELECT count(*) FROM tv_post WHERE fk_user = 1) = $C1
                THEN 'DRY_OK' ELSE 'DRY_FAIL' END"
}

record_env
echo "== skewed_fanout: users=$USERS posts=$POSTS comments=$COMMENTS celebrities=$CELEB"
db_fresh "$TEMPLATE"
tmp="$(mktemp)"
base_sql >"$tmp"
run_script "$TEMPLATE" "$tmp" "$RUN_DIR/skewed_fanout.data.log" || exit 1
[[ $DRY_RUN == 1 ]] || fanout_csv "$TEMPLATE"

for mode in $MODES; do
  echo "== skewed_fanout ($mode)"
  db_fresh "$DB" "$TEMPLATE"
  bench_install "$DB"
  tview_sql "$mode" >"$tmp"
  run_script "$DB" "$tmp" "$RUN_DIR/skewed_fanout_${mode}.setup.log" || exit 1
  if [[ $DRY_RUN == 1 ]]; then
    res="$(dry_run_check "$DB")"; echo "  $res"
    [[ "$res" == DRY_OK ]] || exit 1
    continue
  fi
  workload_sql "$DB" >"$tmp"
  run_script "$DB" "$tmp" "$RUN_DIR/skewed_fanout_${mode}.log" || exit 1
  phys_dump "$DB" skewed_fanout "$mode" || exit 1
  echo "UPDATE tb_user SET name = name || '!' WHERE pk_user = $(users_at "$DB" 0.99 1);" >"$tmp"
  explain_pass "$DB" "$tmp" "skewed_fanout_${mode}" 'tv_(post|comment)'
done
rm -f "$tmp"
db_exec "DROP DATABASE IF EXISTS $DB"
db_exec "DROP DATABASE IF EXISTS $TEMPLATE"
echo "skewed_fanout done -> $RUN_DIR"
