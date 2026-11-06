#!/usr/bin/env bash
# Check that an install of the previous release, upgraded to the commit under test,
# has exactly the catalog of a fresh install and keeps its TVIEWs working
# (ADR 0136, issue #137). Run in two steps around swapping the installed package:
#
#   test/upgrade/upgrade_check.sh before            # previous release installed
#   ...stop PostgreSQL, install the commit under test, start...
#   test/upgrade/upgrade_check.sh after update       # ALTER EXTENSION pg_tviews UPDATE
#   test/upgrade/upgrade_check.sh after recreate     # an older release: drop and recreate
#
# Honors PGHOST/PGPORT/PGUSER. Uses the databases pg_tviews_upgrade and
# pg_tviews_fresh, which it drops and re-creates.

set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
upgraded=pg_tviews_upgrade
fresh=pg_tviews_fresh
fail() { echo "FAIL: $*" >&2; exit 1; }
# Catalogs the update refuses (fixtures_refused.sql), and what its error names.
current="$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/../../Cargo.toml" | head -1)"
refused=(typename_clash lost_privilege)
declare -A refusal=(
    [typename_clash]="these are shared: Post (tv_post, tv_post_)"
    [lost_privilege]="tv_lost (upgrade_lost_owner lacks SELECT on "
)

case "${1:-}" in
before)
    psql -X -d postgres -qc "DROP DATABASE IF EXISTS $upgraded" >/dev/null
    psql -X -d postgres -qc "CREATE DATABASE $upgraded" >/dev/null
    psql -X -d "$upgraded" -q -f "$here/fixtures.sql"
    psql -X -d "$upgraded" -q -f "$here/verify.sql"
    previous="$(psql -X -At -d "$upgraded" -c \
        "SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews'")"
    echo "fixtures created with pg_tviews $previous"
    # The rules the update applies to an existing catalog (an older release is
    # dropped and recreated instead): in the upgraded database, and one database
    # per catalog the update must refuse.
    if [[ -f "$here/../../sql/pg_tviews--${previous}--${current}.sql" ]]; then
        psql -X -d "$upgraded" -q -f "$here/fixtures_security.sql"
        for case in "${refused[@]}"; do
            psql -X -d postgres -qc "DROP DATABASE IF EXISTS ${upgraded}_$case" >/dev/null
            psql -X -d postgres -qc "CREATE DATABASE ${upgraded}_$case" >/dev/null
            psql -X -d "${upgraded}_$case" -q -v case="$case" -f "$here/fixtures_refused.sql"
        done
        echo "security fixtures and refused catalogs created"
    fi
    ;;
after)
    mode="${2:-}"
    # Every C symbol the previous catalog binds is exported by the new library: a
    # call in the window reaches pg_tviews' code (and its revision error), never
    # PostgreSQL's "could not find function … in file".
    # An older catalog is dropped, not updated: only the previous release's counts.
    so="${PG_TVIEWS_SO:-$(pg_config --pkglibdir)/pg_tviews.so}"
    if [[ "$mode" == update && -f "$so" ]]; then
        exported="$(nm -D --defined-only "$so" | awk '{print $NF}')"
        missing="$(psql -X -At -d "$upgraded" -c \
            "SELECT DISTINCT prosrc FROM pg_catalog.pg_proc
              WHERE probin LIKE '%pg_tviews' ORDER BY 1" \
            | while read -r symbol; do grep -qx "$symbol" <<<"$exported" || echo "$symbol"; done)"
        [[ -z "$missing" ]] || fail "symbols the previous catalog binds are missing: $missing"
        echo "every symbol the previous catalog binds is exported"
    fi

    # The new library refuses the previous catalog until it is upgraded (when the
    # catalog revision changed; a 0.1.0 catalog always differs): each kind of write
    # (row trigger, statement trigger, a partition, TRUNCATE) and each function.
    revision_changed=""
    for write in "UPDATE tb_user SET name = name WHERE pk_user = 1" \
                 "UPDATE tb_post SET title = title" \
                 "INSERT INTO tb_event (pk_event, kind) VALUES (2, 'c')" \
                 "TRUNCATE tb_comment"; do
        [[ "$write" == *tb_event* ]] && [[ -z "$(psql -X -At -d "$upgraded" -c \
            "SELECT 1 FROM pg_catalog.pg_class WHERE relname = 'tb_event'")" ]] && continue
        # A release before the TRUNCATE trigger runs no pg_tviews code on TRUNCATE.
        [[ "$write" == TRUNCATE* ]] && [[ -z "$(psql -X -At -d "$upgraded" -c \
            "SELECT 1 FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid
              WHERE t.tgrelid = 'tb_comment'::pg_catalog.regclass
                AND p.proname = 'pg_tview_truncate_trigger'")" ]] && continue
        if out="$(psql -X -d "$upgraded" -v ON_ERROR_STOP=1 -c "BEGIN" -c "$write" \
                -c "ROLLBACK" 2>&1)"; then
            [[ "$mode" == update ]] || fail "a write succeeded against an older catalog"
            [[ -z "$revision_changed" ]] || fail "$write succeeded in the window"
        else
            grep -q "catalog revision" <<<"$out" || fail "unexpected error for $write: $out"
            [[ "$mode" != recreate ]] || grep -q "DROP EXTENSION pg_tviews CASCADE" <<<"$out" \
                || fail "the guard's hint does not say to drop and recreate: $out"
            revision_changed=yes
        fi
    done
    if [[ -z "$revision_changed" ]]; then
        echo "catalog revision unchanged: writes work before ALTER EXTENSION"
    else
        echo "writes refused until the upgrade (row, statement, partition, TRUNCATE)"
    fi
    if [[ -n "$revision_changed" && "$mode" == update ]]; then
        # Every function of the old catalog callable without arguments either works
        # or raises pg_tviews' own error.
        for fn in $(psql -X -At -d "$upgraded" -c \
                "SELECT p.oid::pg_catalog.regproc FROM pg_catalog.pg_proc p
                  WHERE p.probin LIKE '%pg_tviews' AND p.pronargs = p.pronargdefaults
                    AND p.prorettype NOT IN ('pg_catalog.trigger'::pg_catalog.regtype,
                                             'pg_catalog.event_trigger'::pg_catalog.regtype)
                  ORDER BY 1"); do
            out="$(psql -X -d "$upgraded" -c "BEGIN" -c "SELECT * FROM $fn()" \
                -c "ROLLBACK" 2>&1 || true)"
            ! grep -q "could not find function" <<<"$out" || fail "$fn: $out"
        done
        echo "every old function reaches pg_tviews' code in the window"
    fi

    case "$mode" in
    recreate)
        # docs/operations/upgrade.md: no update reaches this release from an older
        # one. Drop the extension and what is left of its TVIEWs, then create them
        # again from their definitions; the base tables keep their rows.
        if psql -X -d "$upgraded" -c "ALTER EXTENSION pg_tviews UPDATE" >/dev/null 2>&1; then
            fail "ALTER EXTENSION UPDATE succeeded from a release without an update path"
        fi
        rows="$(psql -X -At -d "$upgraded" -c "SELECT count(*) FROM tb_user")"
        psql -X -q -d "$upgraded" -v ON_ERROR_STOP=1 <<'SQL'
SET client_min_messages TO WARNING;
DROP EXTENSION pg_tviews CASCADE;
DROP SCHEMA IF EXISTS tviews CASCADE;
-- What is left of the TVIEWs: their tables, and backing views of releases
-- that kept them beside the table.
SELECT format('DROP %s IF EXISTS %I.%I CASCADE',
              CASE c.relkind WHEN 'v' THEN 'VIEW' ELSE 'TABLE' END, n.nspname, c.relname)
  FROM pg_catalog.pg_class c
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
 WHERE n.nspname IN ('public', 'app', 'scratch') AND c.relkind IN ('r', 'v')
   AND (c.relname LIKE 'tv\_%' OR c.relname LIKE 'v\_%') \gexec
CREATE EXTENSION pg_tviews;
SQL
        [[ "$(psql -X -At -d "$upgraded" -c "SELECT count(*) FROM tb_user")" == "$rows" ]] \
            || fail "dropping the extension changed tb_user"
        psql -X -q -d "$upgraded" -v ON_ERROR_STOP=1 <<'SQL'
SET client_min_messages TO WARNING;
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
UPDATE tb_user SET name = name || '+' WHERE pk_user = 1;
DO $$ BEGIN
    IF EXISTS (SELECT pk_post, data FROM tv_post EXCEPT
               SELECT p.pk_post, jsonb_build_object('title', p.title, 'author', u.data)
                 FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user) THEN
        RAISE EXCEPTION 'upgrade check: tv_post stale after the recreate';
    END IF;
END $$;
SQL
        echo "dropped and recreated: the TVIEWs follow their base tables"
        ;;
    update)
        start=$(date +%s%N)
        out="$(psql -X -d "$upgraded" -v ON_ERROR_STOP=1 -c "ALTER EXTENSION pg_tviews UPDATE" 2>&1)" \
            || fail "ALTER EXTENSION pg_tviews UPDATE: $out"
        echo "$out"
        echo "ALTER EXTENSION pg_tviews UPDATE took $(( ($(date +%s%N) - start) / 1000000 )) ms"
        # The update re-derived every TVIEW: none is left to re-register.
        left="$(psql -X -At -d "$upgraded" -c \
            "SELECT count(*) FROM tviews.registry WHERE needs_reregister")"
        [[ "$left" == 0 ]] || fail "$left TVIEW(s) left to re-register after the update"
        # A DISTINCT ON TVIEW is keyed on its DISTINCT ON key (ADR 0169): the
        # unique index on pk_<entity> it had before is gone.
        left="$(psql -X -At -d "$upgraded" -c \
            "SELECT count(*) FROM pg_catalog.pg_class WHERE relkind = 'i' AND relname LIKE 'idx\_tv\_%\_pk\_unique'")"
        [[ "$left" == 0 ]] || fail "$left pk_unique index(es) survived re-registration"
        psql -X -d "$upgraded" -q -f "$here/verify_after.sql"
        if [[ -n "$(psql -X -At -d "$upgraded" -c "SELECT 1 FROM pg_namespace WHERE nspname = 'sec'")" ]]; then
            grep -q "tv_tag runs sec.label(text), whose search_path (sec) does not end in pg_temp" <<<"$out" \
                || fail "the update did not warn about sec.label: $out"
            psql -X -d "$upgraded" -q -f "$here/verify_security.sql"
            echo "the update re-derived as owners, revoked column grants, reported function paths"
            for case in "${refused[@]}"; do
                if out="$(psql -X -d "${upgraded}_$case" -v ON_ERROR_STOP=1 \
                        -c "ALTER EXTENSION pg_tviews UPDATE" 2>&1)"; then
                    fail "the update accepted the $case catalog"
                fi
                grep -qF -- "${refusal[$case]}" <<<"$out" || fail "the $case refusal: $out"
                [[ "$(psql -X -At -d "${upgraded}_$case" -c \
                    "SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews'")" != "$current" ]] \
                    || fail "the refused $case update changed the extension's version"
                psql -X -d postgres -qc "DROP DATABASE ${upgraded}_$case" >/dev/null
            done
            echo "the update refused the catalogs it must: ${refused[*]}"
        fi
        ;;
    *) fail "usage: $0 after update|recreate" ;;
    esac

    psql -X -d postgres -qc "DROP DATABASE IF EXISTS $fresh" >/dev/null
    psql -X -d postgres -qc "CREATE DATABASE $fresh" >/dev/null
    psql -X -d "$fresh" -q -v ON_ERROR_STOP=1 \
        -c "SET client_min_messages TO WARNING" \
        -c "CREATE EXTENSION jsonb_delta" -c "CREATE EXTENSION pg_tviews"

    tmp="$(mktemp -d)"
    psql -X -At -d "$upgraded" -f "$here/catalog_snapshot.sql" >"$tmp/upgraded"
    psql -X -At -d "$fresh" -f "$here/catalog_snapshot.sql" >"$tmp/fresh"
    if ! diff -u "$tmp/fresh" "$tmp/upgraded"; then
        fail "the upgraded catalog differs from a fresh install (- fresh, + upgraded)"
    fi
    echo "catalog identical to a fresh install ($(wc -l <"$tmp/fresh") facts)"

    if [[ "$mode" == update ]]; then
        # The tables carry the triggers the same TVIEWs get on a fresh install:
        # one set per table, the per-TVIEW triggers of older releases gone.
        reference=pg_tviews_upgrade_reference
        psql -X -d postgres -qc "DROP DATABASE IF EXISTS $reference" >/dev/null
        psql -X -d postgres -qc "CREATE DATABASE $reference" >/dev/null
        psql -X -d "$reference" -q -f "$here/fixtures.sql"
        psql -X -d "$reference" -q -f "$here/verify.sql"
        triggers="SELECT pg_catalog.pg_get_triggerdef(t.oid) || ' ' || t.tgenabled::pg_catalog.text
                    FROM pg_catalog.pg_trigger t
                    JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid
                   WHERE p.pronamespace = 'tviews'::pg_catalog.regnamespace
                     AND p.proname IN ('pg_tview_trigger_handler', 'pg_tview_flush_trigger',
                                       'pg_tview_delta_trigger', 'pg_tview_truncate_trigger')
                     AND t.tgparentid = 0
                     -- The security fixtures (fixtures_security.sql) have no reference.
                     AND t.tgrelid NOT IN (SELECT c.oid FROM pg_catalog.pg_class c
                                           WHERE c.relnamespace = pg_catalog.to_regnamespace('sec'))
                   ORDER BY 1"
        psql -X -At -d "$upgraded" -c "$triggers" >"$tmp/upgraded_triggers"
        psql -X -At -d "$reference" -c "$triggers" >"$tmp/fresh_triggers"
        if ! diff -u "$tmp/fresh_triggers" "$tmp/upgraded_triggers"; then
            fail "the upgraded tables' triggers differ from a fresh install (- fresh, + upgraded)"
        fi
        echo "base-table triggers identical to a fresh install ($(wc -l <"$tmp/fresh_triggers") triggers)"
        psql -X -d postgres -qc "DROP DATABASE $reference" >/dev/null

        psql -X -d "$upgraded" -q -f "$here/verify.sql"
        echo "TVIEWs follow their base tables after the upgrade"
        psql -X -d "$upgraded" -q -f "$here/verify_privileges.sql"
        echo "readers read the TVIEW tables; backing views are their owners' alone"
    fi
    ;;
*)
    fail "usage: $0 before | after update|recreate"
    ;;
esac
