#!/usr/bin/env bash
# Build the template databases the read-path benchmark copies, from seed.sql:
#   rc_embedded$SUFFIX      tv_post.data.comments[] (the #228 array shape)
#   rc_composed$SUFFIX      tv_post without comments, plus tv_comment
#   rc_composed_key$SUFFIX  rc_composed with the post's key inside each comment (key.sql)
# Then check that both designs hold the same documents: the md5 over every post with
# its comments, composed from tv_comment, equals the md5 over the embedded posts.
# Usage: PGHOST=… PGPORT=… PGUSER=… [POSTS=2000 SUFFIX=_50] ./setup.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
P="${POSTS:-20000}"; S="${SUFFIX:-}"

for design in embedded composed; do
  db="rc_$design$S"
  psql -X -q -d postgres -c "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $db" \
       -c "CREATE DATABASE $db" >/dev/null
  psql -X -q -v ON_ERROR_STOP=1 -v design="$design" -v posts="$P" -d "$db" -f "$here/seed.sql" >/dev/null
done
psql -X -q -d postgres -c "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS rc_composed_key$S" \
     -c "CREATE DATABASE rc_composed_key$S TEMPLATE rc_composed$S STRATEGY FILE_COPY" >/dev/null
psql -X -q -v ON_ERROR_STOP=1 -d "rc_composed_key$S" -f "$here/key.sql" >/dev/null

embedded="$(psql -X -At -d "rc_embedded$S" -c \
  "SELECT md5(string_agg(data::text, '' ORDER BY pk_post)) FROM bench.tv_post")"
composed="$(psql -X -At -d "rc_composed$S" -c \
  "SELECT md5(string_agg((p.data || jsonb_build_object('comments', COALESCE(
              (SELECT jsonb_agg(c.data ORDER BY c.pk_comment) FROM bench.tv_comment c
               WHERE c.fk_post = p.pk_post), '[]'::jsonb)))::text, '' ORDER BY p.pk_post))
   FROM bench.tv_post p")"
if [[ "$embedded" != "$composed" ]]; then
  echo "FAIL: the designs hold different documents (embedded $embedded, composed $composed)" >&2
  exit 1
fi
echo "rc_embedded$S, rc_composed$S, rc_composed_key$S: $P posts, same documents ($embedded)"
