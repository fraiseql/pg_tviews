-- Concurrent differential fuzzer: what run.sh loads after the differential
-- harness (../../sql/differential/schema.sql). Adds the shapes of #263 (users who
-- comment on each other's posts, read by a join and by embedded TVIEWs), the
-- transaction generator the clients call, and the check the checker runs.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

-- ── #263: crossed fan-outs ──────────────────────────────────────────────────
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, bio text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author int NOT NULL, title text);
CREATE TABLE tb_comment (pk_comment int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                         id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post int NOT NULL, fk_author int NOT NULL, content text);
INSERT INTO tb_user (pk_user, name, bio) SELECT g, 'u' || g, '' FROM generate_series(1, 8) g;
INSERT INTO tb_post (pk_post, fk_author, title) SELECT g, 1 + g % 8, 'p' || g FROM generate_series(1, 16) g;
-- every user comments on posts of the others
INSERT INTO tb_comment (fk_post, fk_author, content)
SELECT p, 1 + (p + a) % 8, 'c' || p || '_' || a FROM generate_series(1, 16) p, generate_series(1, 3) a;
CREATE INDEX ON tb_post (fk_author);
CREATE INDEX ON tb_comment (fk_post);
CREATE INDEX ON tb_comment (fk_author);

-- tv_post reads users twice through joins, grouped by its identity
SELECT harness_create('tv_post', 'pk_post', $$
  SELECT p.pk_post, p.id,
         jsonb_build_object('title', p.title,
           'author', jsonb_build_object('name', u.name, 'bio', u.bio),
           'comments', COALESCE(jsonb_agg(jsonb_build_object('content', c.content,
               'author', jsonb_build_object('name', cu.name, 'bio', cu.bio))
             ORDER BY c.pk_comment) FILTER (WHERE c.pk_comment IS NOT NULL), '[]')) AS data
  FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author
  LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
  LEFT JOIN tb_user cu ON cu.pk_user = c.fk_author
  GROUP BY p.pk_post, p.id, p.title, u.name, u.bio $$);
-- the same documents built from embedded TVIEWs
SELECT harness_create('tv_author', 'pk_author', $$
  SELECT u.pk_user AS pk_author, u.id, jsonb_build_object('name', u.name, 'bio', u.bio) AS data
  FROM tb_user u $$);
SELECT harness_create('tv_remark', 'pk_remark', $$
  SELECT c.pk_comment AS pk_remark, c.id, c.fk_post, c.fk_author AS fk_author,
         jsonb_build_object('content', c.content, 'author', a.data) AS data
  FROM tb_comment c JOIN tv_author a ON a.pk_author = c.fk_author $$);
SELECT harness_create('tv_article', 'pk_article', $$
  SELECT p.pk_post AS pk_article, p.id, p.fk_author,
         jsonb_build_object('title', p.title, 'author', a.data,
           'remarks', COALESCE((SELECT jsonb_agg(r.data ORDER BY r.pk_remark)
                                FROM tv_remark r WHERE r.fk_post = p.pk_post), '[]')) AS data
  FROM tb_post p JOIN tv_author a ON a.pk_author = p.fk_author $$);

-- ── generator ───────────────────────────────────────────────────────────────
-- One statement of the #263 schema: single-row value-only writes (`links` false)
-- or any.
CREATE FUNCTION fuzz_crossed(links boolean) RETURNS text LANGUAGE plpgsql AS $$
DECLARE r int := floor(random() * CASE WHEN links THEN 8 ELSE 3 END)::int;
BEGIN
    RETURN CASE r
    WHEN 0 THEN format('UPDATE tb_user SET bio = %L WHERE pk_user = %s', md5(random()::text), harness_i(8))
    WHEN 1 THEN format('UPDATE tb_post SET title = %L WHERE pk_post = %s', md5(random()::text), harness_i(16))
    -- one row: two statements updating several base rows lock them in their scan
    -- orders, which deadlocks without pg_tviews too
    WHEN 2 THEN format('UPDATE tb_comment SET content = %L WHERE pk_comment IN %s', md5(random()::text),
                       harness_pick('tb_comment', 'pk_comment', CASE WHEN links THEN harness_n() ELSE 1 END))
    WHEN 3 THEN format('INSERT INTO tb_comment (fk_post, fk_author, content) VALUES (%s, %s, ''n'')', harness_i(16), harness_i(8))
    WHEN 4 THEN format('UPDATE tb_comment SET fk_post = %s WHERE pk_comment IN %s', harness_i(16),
                       harness_pick('tb_comment', 'pk_comment', 1))
    WHEN 5 THEN format('DELETE FROM tb_comment WHERE pk_comment IN %s', harness_pick('tb_comment', 'pk_comment', 1))
    WHEN 6 THEN format('UPDATE tb_post SET fk_author = %s WHERE pk_post = %s', harness_i(8), harness_i(16))
    ELSE format('UPDATE tb_user SET name = name || ''.'' WHERE pk_user = %s', harness_i(8))
    END;
END $$;

-- Run `stmt` in a subtransaction: a constraint it violates (rows another client
-- deleted or created meanwhile) only undoes it; `undo` rolls it back on purpose
-- (ROLLBACK TO SAVEPOINT). Anything else, a serialization failure or a deadlock
-- included, fails the transaction.
CREATE FUNCTION fuzz_exec(stmt text, undo boolean) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    IF undo THEN
        RAISE EXCEPTION USING ERRCODE = 'P0004', MESSAGE = 'undo';
    END IF;
EXCEPTION
    WHEN integrity_constraint_violation OR assert_failure THEN NULL;
END $$;

-- One transaction's statements, seeded by `seed`: 1 to 4 of the differential
-- harness's (`harness` percent of them) or the #263 schema's, one in five rolled
-- back to its savepoint.
CREATE FUNCTION fuzz_txn(seed int, harness int) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM setseed(seed / 2147483648.0);
    FOR i IN 1..1 + floor(random() * 4)::int LOOP
        PERFORM fuzz_exec(CASE WHEN random() * 100 < harness THEN harness_statement(seed % 100000)
                               ELSE fuzz_crossed(true) END,
                          random() < 0.2);
    END LOOP;
END $$;

-- The table's and the view's version of the first row they disagree on.
CREATE FUNCTION fuzz_rows(tv regclass, key text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE
    v regclass;
    out text;
BEGIN
    SELECT m.view_oid::oid::regclass INTO v FROM tviews.pg_tview_meta m WHERE m.table_oid::oid = tv::oid;
    EXECUTE format('SELECT format(E''\n    table: %%s\n    view:  %%s'', t.data, w.data)
                    FROM %1$s t FULL JOIN %2$s w USING (%3$I)
                    WHERE t.data IS DISTINCT FROM w.data ORDER BY 1 LIMIT 1', tv, v, key) INTO out;
    RETURN coalesce(out, '');
END $$;

-- What the checker runs, in one REPEATABLE READ snapshot: every TVIEW equals its
-- view, since every committed transaction maintained its rows. While writers run
-- (`during`), a TVIEW over a materialized view is skipped: REFRESH MATERIALIZED
-- VIEW writes its new rows frozen, so a snapshot older than the REFRESH reads them
-- next to the TVIEW rows of before it (PostgreSQL's MVCC-unsafe rewrite).
CREATE FUNCTION fuzz_check(during boolean) RETURNS text LANGUAGE plpgsql AS $$
DECLARE
    s record;
    diff text;
    failures text := '';
BEGIN
    FOR s IN SELECT h.tv, h.key FROM harness_shape h
             WHERE NOT (during AND EXISTS (
                 SELECT 1 FROM tviews.pg_tview_meta m, pg_catalog.pg_depend d
                 JOIN pg_catalog.pg_rewrite rw ON d.classid = 'pg_catalog.pg_rewrite'::regclass AND rw.oid = d.objid
                 JOIN pg_catalog.pg_class c ON c.oid = d.refobjid
                 WHERE m.table_oid::oid = h.tv::oid AND rw.ev_class = m.view_oid::oid AND c.relkind = 'm'))
             ORDER BY h.tv::text LOOP
        diff := fresh_diff(s.tv, s.key);
        IF diff IS NOT NULL THEN
            failures := failures || E'\n  ' || diff || fuzz_rows(s.tv, s.key);
        END IF;
    END LOOP;
    RETURN nullif(failures, '');
END $$;
