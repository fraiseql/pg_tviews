#!/usr/bin/env bash
# F8 (#94): payload sweep under storage knobs. Same workload as payload_sweep.sh
# (200 single-row one-integer updates), varying how tv_doc.data is stored.
#   KNOBS   "default main lz4 main_lz4 tuple_target"
#   PAYLOAD "random" (md5 hex, incompressible) | "text" (compressible JSON-ish)
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$here/../lib/common.sh"
SIZES="${SIZES:-4096 16384 65536}"
ROWS="${ROWS:-2000}"; UPDATES="${UPDATES:-200}"
KNOBS="${KNOBS:-default main lz4 main_lz4 tuple_target}"
PAYLOAD="${PAYLOAD:-random}"
MODES="${MODES:-logged}"

payload_expr() {
  if [[ $PAYLOAD == random ]]; then
    echo "left((SELECT string_agg(md5(g::text || ':' || s), '') FROM generate_series(1, ceil($1 / 32.0)::int) s), $1)"
  else
    echo "left((SELECT string_agg('{\"field\": \"value ' || (s % 50) || '\", \"note\": \"lorem ipsum dolor sit amet ' || (g % 7) || '\"}', ', ') FROM generate_series(1, ceil($1 / 70.0)::int) s), $1)"
  fi
}
knob_sql() {
  case "$1" in
    default) ;;
    main) echo "ALTER TABLE tv_doc ALTER COLUMN data SET STORAGE MAIN;" ;;
    lz4) echo "ALTER TABLE tv_doc ALTER COLUMN data SET COMPRESSION lz4;" ;;
    main_lz4) echo "ALTER TABLE tv_doc ALTER COLUMN data SET STORAGE MAIN; ALTER TABLE tv_doc ALTER COLUMN data SET COMPRESSION lz4;" ;;
    tuple_target) echo "ALTER TABLE tv_doc SET (toast_tuple_target = 8160);" ;;
    extended_lz4_ext) echo "ALTER TABLE tv_doc ALTER COLUMN data SET STORAGE EXTERNAL;" ;;
  esac
}
record_env
for mode in $MODES; do for knob in $KNOBS; do for size in $SIZES; do
  scenario="toast_${PAYLOAD}_${knob}_${size}"; db=bench_phys_toast
  echo "== $scenario ($mode)"
  db_fresh "$db"; bench_install "$db"; tmp="$(mktemp)"
  cat >"$tmp" <<SQL
SET client_min_messages TO WARNING;
CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     counter int NOT NULL DEFAULT 0, payload text NOT NULL);
INSERT INTO tb_doc (pk_doc, payload) SELECT g, $(payload_expr "$size") FROM generate_series(1, $ROWS) g;
$(mode_sql "$mode")
SELECT pg_tviews_create('tv_doc', \$v\$
  SELECT pk_doc, id, jsonb_build_object('counter', counter, 'payload', payload) AS data FROM tb_doc \$v\$);
$(knob_sql "$knob")
VACUUM FULL tv_doc;
SQL
  run_script "$db" "$tmp" "$RUN_DIR/${scenario}.setup.log" || exit 1
  {
    phys_step_begin
    step=$(( ROWS / UPDATES ))
    for ((i = 0; i < UPDATES; i++)); do
      echo "UPDATE tb_doc SET counter = counter + 1 WHERE pk_doc = $(( 1 + (i * step) % ROWS ));"
    done
    phys_snap "update_single" "$UPDATES"
    echo "SELECT count(*) FROM tv_doc WHERE (data->>'counter')::int > 0;"  # read cost probe
    divergence_gate tv_doc v_doc pk_doc
  } >"$tmp"
  run_script "$db" "$tmp" "$RUN_DIR/${scenario}.log" || exit 1
  phys_dump "$db" "$scenario" "$mode" || exit 1
  rm -f "$tmp"
done; done; done
db_exec "DROP DATABASE IF EXISTS bench_phys_toast"
echo "toast_knobs done -> $RUN_DIR"
