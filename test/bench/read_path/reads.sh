#!/usr/bin/env bash
# Read cost of post.comments, embedded (tv_post.data.comments) against composed at read
# time from tv_comment (LATERAL in four spellings, or a batched fk_post = ANY($1) second
# query). Templates: setup.sh.
# Server-side: each cell is a fresh FILE_COPY of its design's database, a warm-up loop,
# then a measured plpgsql loop; every document is cast to text (what the wire carries).
#   one:    1 post by id
#   page20: 20 posts, keyset page (pk_post > k ORDER BY pk_post LIMIT 20)
#   any20:  20 posts by 20 random ids (a nested list: no heap locality between them)
# Usage: PGHOST=… PGPORT=… PGUSER=… [POSTS=2000 SUFFIX=_50] ./reads.sh [rounds]
set -euo pipefail
# shellcheck source=../write_path/lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../write_path/lib.sh"
rounds="${1:-3}"
# POSTS: the seed's post count (seed.sql -v posts); templates rc_<design>$SUFFIX.
P="${POSTS:-20000}"; S="${SUFFIX:-}"
db=rc_run
trap 'psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT


# The parent rows of one iteration, as a query over bench.tv_post p, per size.
where_of() {
  case "$1" in
    one) echo "p.id = ids[1 + (i % $P)]";;
    page20) echo "p.pk_post > (i * 397) % ($P - 20) ORDER BY p.pk_post LIMIT 20";;
    any20) echo "p.id = ANY (ids[1 + (i * 20) % $P : 20 + (i * 20) % $P])";;
  esac
}

# The document of one post with its comments, composed from tv_comment, per spelling.
#   lateral:      jsonb_agg, merged into the post's jsonb (||)
#   lateral_text: json_agg spliced into the post's text: no jsonb rebuilt
#   fraiseql:     the shape fraiseql's composed read renders (fraiseql-db composed.rs),
#                 correlated on the native fk_post column
#   fraiseql_key: the same, correlated on a key in data as fraiseql renders it today,
#                 (data->>'fk_post')::int, with an expression index (rc_composed_key)
compose_of() {
  local agg="FROM bench.tv_comment c WHERE c.fk_post = p.pk_post"
  case "$1" in
    lateral) echo "(p.data || jsonb_build_object('comments', COALESCE((SELECT jsonb_agg(c.data ORDER BY c.pk_comment) $agg), '[]'::jsonb)))::text";;
    lateral_text) echo "left(p.data::text, -1) || ', \"comments\": ' || COALESCE((SELECT json_agg(c.data ORDER BY c.pk_comment) $agg)::text, '[]') || '}'";;
    fraiseql|fraiseql_key)
      local corr="fk_post = p.pk_post"; [[ "$1" == fraiseql_key ]] && corr="(data->>'fk_post')::int = p.pk_post"
      echo "jsonb_build_object('d', p.data, 'e', jsonb_build_object('comments', (SELECT COALESCE(jsonb_agg(_c.data ORDER BY _c._o), '[]'::jsonb) FROM (SELECT data, row_number() OVER (ORDER BY pk_comment) AS _o FROM bench.tv_comment WHERE $corr ORDER BY _o) _c)))::text";;
  esac
}

# The loop body of one variant: reads the documents of one iteration into n (bytes).
body_of() {
  local w; w="$(where_of "$2")"
  case "$1" in
    embedded) cat <<SQL
      FOR r IN SELECT p.data::text AS t FROM bench.tv_post p WHERE $w LOOP n := n + length(r.t); END LOOP;
SQL
    ;;
    lateral|lateral_text|fraiseql|fraiseql_key) cat <<SQL
      FOR r IN SELECT $(compose_of "$1") AS t FROM bench.tv_post p WHERE $w LOOP n := n + length(r.t); END LOOP;
SQL
    ;;
    batched) cat <<SQL
      keys := '{}';
      FOR r IN SELECT p.pk_post AS k, p.data::text AS t FROM bench.tv_post p WHERE $w LOOP
        keys := keys || r.k; n := n + length(r.t);
      END LOOP;
      FOR r IN SELECT c.fk_post AS k, c.data::text AS t FROM bench.tv_comment c
               WHERE c.fk_post = ANY (keys) ORDER BY c.fk_post, c.pk_comment LOOP
        n := n + length(r.t);
      END LOOP;
SQL
    ;;
  esac
}

loop_sql() { # <variant> <size> <iterations> <label>
  cat <<SQL
DO \$\$ DECLARE ids uuid[]; keys int[]; r record; n bigint := 0; t0 timestamptz; BEGIN
  SELECT array_agg(id ORDER BY md5(id::text)) INTO ids FROM bench.tv_post;
  t0 := clock_timestamp();
  FOR i IN 1..$3 LOOP
$(body_of "$1" "$2")
  END LOOP;
  RAISE NOTICE '$4 % %', round(extract(epoch FROM clock_timestamp() - t0) * 1e6 / $3, 1), n / $3;
END \$\$;
SQL
}

iters() { [[ "$1" == one ]] && echo 20000 || echo 4000; }

printf '%-6s %-8s %-13s %10s %10s\n' round size variant us/read bytes/read
for round in $(seq 1 "$rounds"); do
  for size in one page20 any20; do
    for variant in embedded lateral lateral_text fraiseql fraiseql_key batched; do
      tpl=rc_composed$S; [[ "$variant" == embedded ]] && tpl=rc_embedded$S
      [[ "$variant" == fraiseql_key ]] && tpl=rc_composed_key$S
      fresh_copy "$tpl" "$db"
      n="$(iters "$size")"
      out="$( { loop_sql "$variant" "$size" $(( n / 2 )) warm; loop_sql "$variant" "$size" "$n" measured; } \
              | psql -X -q -v ON_ERROR_STOP=1 -d "$db" 2>&1 | sed -n 's/.*NOTICE:  measured //p')"
      read -r us bytes <<<"$out"
      printf '%-6s %-8s %-13s %10s %10s\n' "$round" "$size" "$variant" "$us" "$bytes"
    done
  done
done
