-- A restored database keeps refreshing its TVIEWs when a base table had dropped
-- columns (architecture review A2).
--
-- The stored plan names columns by attribute number ({c:<relid>:<attnum>},
-- `attnums`, `links`, `initial_attnum`, the read sets' `attnum`). A table with
-- dropped columns keeps holes in its attribute numbers; pg_dump writes it without
-- them, so the restored columns have other numbers. The restore must rebind each
-- one by its name, as it rebinds the relids: else a write after the restore fails
-- (42883, 22P02) or, where the shifted number lands on a column of the same type,
-- refreshes the wrong rows silently.
--
-- Round trips through pg_dump -Fp (restored with psql) and pg_dump -Fc (pg_restore
-- --exit-on-error), shelling out as regress_dump_restore.sql does.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_restore_dropped_columns.sql
-- expect-output: restore_dropped_columns: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

SELECT current_setting('server_version_num')::int / 10000 AS server_major \gset
\setenv PGTV_SERVER_MAJOR :server_major
\! test "$(pg_dump --version | sed -E 's/^[^0-9]*([0-9]+).*/\1/')" = "$PGTV_SERVER_MAJOR"
\if :SHELL_ERROR
  \echo 'SKIP: pg_dump on PATH is not the server''s major version'
  \quit
\endif
\set dumpdir `mktemp -d`
\setenv PGTV_DUMPDIR :dumpdir

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- Holes before every column a TVIEW reads.
CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    junk1 TEXT, junk2 TEXT,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT, email TEXT);
ALTER TABLE tb_user DROP COLUMN junk1, DROP COLUMN junk2;
-- fk_user moves from attnum 3 to 2 and fk_editor from 4 to 3: a stale fk_user
-- reads fk_editor, of the same type (the silent variant).
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    junk INTEGER,
    fk_user INTEGER NOT NULL REFERENCES tb_user (pk_user),
    fk_editor INTEGER NOT NULL REFERENCES tb_user (pk_user),
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT);
ALTER TABLE tb_post DROP COLUMN junk;
INSERT INTO tb_user (name, email) VALUES ('ann', 'a@x'), ('bob', 'b@x'), ('cid', 'c@x');
INSERT INTO tb_post (fk_user, fk_editor, title) VALUES (1, 3, 'p1'), (2, 3, 'p2'), (1, 2, 'p3');

CREATE TABLE tv_user AS
SELECT pk_user, id, jsonb_build_object('id', id, 'name', name, 'email', email) AS data
FROM tb_user;
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_user, p.fk_editor,
       jsonb_build_object('id', p.id, 'title', p.title,
                          'author_name', u.name, 'author_email', u.email) AS data
FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user;

-- Writes after the restore, each followed by a comparison of both TVIEWs with
-- their views. Restored with the database.
CREATE FUNCTION a2_check(label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    bad bigint;
BEGIN
    SELECT count(*) INTO bad FROM (
        (SELECT pk_post, id, fk_user, fk_editor, data FROM tv_post
         EXCEPT SELECT pk_post, id, fk_user, fk_editor, data FROM tviews.public__tv_post)
        UNION ALL
        (SELECT pk_post, id, fk_user, fk_editor, data FROM tviews.public__tv_post
         EXCEPT SELECT pk_post, id, fk_user, fk_editor, data FROM tv_post)
        UNION ALL
        (SELECT pk_user, id, NULL::int, NULL::int, data FROM tv_user
         EXCEPT SELECT pk_user, id, NULL::int, NULL::int, data FROM tviews.public__tv_user)
        UNION ALL
        (SELECT pk_user, id, NULL::int, NULL::int, data FROM tviews.public__tv_user
         EXCEPT SELECT pk_user, id, NULL::int, NULL::int, data FROM tv_user)) d;
    IF bad <> 0 THEN
        RAISE EXCEPTION 'A2 FAIL (%): % rows differ from the views', label, bad;
    END IF;
END $$;

CREATE FUNCTION a2_writes() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_user SET name = 'ann2' WHERE pk_user = 1;          -- fan-out to tv_post
    PERFORM a2_check('update tb_user.name');
    INSERT INTO tb_post (fk_user, fk_editor, title) VALUES (2, 1, 'p4');
    PERFORM a2_check('insert into tb_post');
    UPDATE tb_post SET title = 'p1b' WHERE pk_post = 1;
    PERFORM a2_check('update tb_post.title');
    UPDATE tb_post SET fk_user = 3 WHERE pk_post = 3;            -- a link moves
    PERFORM a2_check('update tb_post.fk_user');
    UPDATE tb_user SET email = 'c2@x' WHERE pk_user = 3;         -- reaches the moved row
    PERFORM a2_check('update tb_user.email');
    DELETE FROM tb_post WHERE pk_post = 2;
    PERFORM a2_check('delete from tb_post');
END $$;

SELECT a2_check('source');

\setenv PGTV_SRC :DBNAME
\! dropdb --if-exists "${PGTV_SRC}_fp" && createdb "${PGTV_SRC}_fp" && pg_dump -Fp -f "$PGTV_DUMPDIR/src.sql" "$PGTV_SRC" && psql -X -q -v ON_ERROR_STOP=1 -d "${PGTV_SRC}_fp" -f "$PGTV_DUMPDIR/src.sql" >/dev/null
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION 'A2 FAIL: the plain dump did not restore'; END $$;
\endif
\! dropdb --if-exists "${PGTV_SRC}_fc" && createdb "${PGTV_SRC}_fc" && pg_dump -Fc -f "$PGTV_DUMPDIR/src.dump" "$PGTV_SRC" && pg_restore --exit-on-error -d "${PGTV_SRC}_fc" "$PGTV_DUMPDIR/src.dump"
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION 'A2 FAIL: the custom-format dump did not restore'; END $$;
\endif

\set fp :DBNAME '_fp'
\set fc :DBNAME '_fc'
\set src :DBNAME

\c :fp
SET client_min_messages TO WARNING;
-- The restored table has no holes: the plan must name its columns' new numbers.
SELECT a2_check('restored -Fp');
SELECT a2_writes();

\c :fc
SET client_min_messages TO WARNING;
SELECT a2_check('restored -Fc');
SELECT a2_writes();

\c :src
SET client_min_messages TO WARNING;
-- The health check reports a plan numbering a column with another's number.
UPDATE tviews.pg_tview_meta
   SET plan = jsonb_set(plan, '{tables,0,attnums,1}', '3')
 WHERE entity = 'user';
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check()
                  WHERE component = 'columns' AND status = 'ERROR'
                    AND message LIKE '%tv_user (public.tb_user.id)%') THEN
    RAISE EXCEPTION 'A2 FAIL: the health check misses a stale column number: %',
      (SELECT array_agg(message) FROM tviews.pg_tviews_health_check() WHERE component = 'columns');
  END IF;
END $$;
SELECT tviews.pg_tviews_reregister('user') \g /dev/null
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check()
                  WHERE component = 'columns' AND status = 'OK') THEN
    RAISE EXCEPTION 'A2 FAIL: the health check still reports stale column numbers after reregister';
  END IF;
END $$;
\! dropdb --if-exists "${PGTV_SRC}_fp"; dropdb --if-exists "${PGTV_SRC}_fc"; rm -rf "$PGTV_DUMPDIR"
\echo 'restore_dropped_columns: PASS'
