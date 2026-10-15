#!/usr/bin/env bash
# Write cost of the two designs of reads.sh, server-side: 400 writes in one DO block per
# cell, each cell a fresh FILE_COPY of its design's database. plain = the write alone
# (session_replication_role = replica, on the composed copy: the same base tables).
# WAL: pg_current_wal_lsn() around the block, per write.
#   username: a user who wrote 50 comments renamed (embedded: their posts' arrays)
#   content:  one comment's text      title:  one post's title
#   insert:   a comment added         move:   a comment moved to another post
# Usage: PGHOST=… PGPORT=… PGUSER=… [POSTS=2000 SUFFIX=_50] ./writes.sh [rounds]
set -euo pipefail
# shellcheck source=../write_path/lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../write_path/lib.sh"
rounds="${1:-3}"
# POSTS: the seed's post count (seed.sql -v posts); templates rc_<design>$SUFFIX.
P="${POSTS:-20000}"; S="${SUFFIX:-}"
db=rc_run
trap 'psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT


write_of() {
  case "$1" in
    username) echo "UPDATE bench.tb_user SET username = 'user' || k || '_' || i WHERE pk_user = 1 + k % 2000;";;
    content) echo "UPDATE bench.tb_comment SET content = 'edited ' || i WHERE pk_comment = 1 + k % 100000;";;
    title) echo "UPDATE bench.tb_post SET title = 'Title ' || k || '_' || i WHERE pk_post = 1 + k % $P;";;
    insert) echo "INSERT INTO bench.tb_comment (fk_post, fk_author, content) VALUES (1 + k % $P, 1 + k % 2000, 'new comment');";;
    move) echo "UPDATE bench.tb_comment SET fk_post = 1 + (k + 7919) % $P WHERE pk_comment = 1 + k % 100000;";;
  esac
}

printf '%-6s %-9s %-9s %10s %12s\n' round write design ms/write wal_B/write
for round in $(seq 1 "$rounds"); do
  for write in username content title insert move; do
    for design in embedded composed plain; do
      tpl=rc_composed$S; [[ "$design" == embedded ]] && tpl=rc_embedded$S
      pre=""; [[ "$design" == plain ]] && pre="SET session_replication_role = replica;"
      fresh_copy "$tpl" "$db"
      out="$(psql -X -q -v ON_ERROR_STOP=1 -d "$db" 2>&1 <<SQL
$pre
SELECT pg_current_wal_lsn() AS l0 \\gset
DO \$\$ DECLARE t0 timestamptz := clock_timestamp(); k int; BEGIN
  FOR i IN 1..400 LOOP
    k := (i * 7919) % 100000;
    $(write_of "$write")
  END LOOP;
  RAISE NOTICE 'ms %', round(extract(epoch FROM clock_timestamp() - t0) * 1000 / 400, 3);
END \$\$;
SELECT round(pg_wal_lsn_diff(pg_current_wal_lsn(), :'l0') / 400) AS wal \\gset
\\echo WAL :wal
SQL
)"
      ms="$(sed -n 's/.*NOTICE:  ms //p' <<<"$out")"; wal="$(sed -n 's/^WAL //p' <<<"$out")"
      printf '%-6s %-9s %-9s %10s %12s\n' "$round" "$write" "$design" "$ms" "$wal"
    done
  done
done
