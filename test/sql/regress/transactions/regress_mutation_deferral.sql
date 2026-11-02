-- Regression test: a mutation refreshes the TVIEWs once (#286).
--
-- A fraiseql mutation is one outer statement, `SELECT app.fn(…)`, whose function
-- writes in several statements. The TVIEWs are refreshed once: when the outermost
-- statement ends, or at the first query inside it that reads a TVIEW's table
-- (directly, through a view, from a STABLE or SQL function, a cursor or RETURN
-- QUERY), whichever comes first. That query sees the refreshed rows. A
-- subtransaction rolled back inside the function takes its queued work with it;
-- a refresh that fails raises at the read, or at the outer statement's end.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_mutation_deferral.sql
--
-- expect-output: mutation deferral: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS md CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'mutation deferral FAIL: %', what; END IF; END $$;
CREATE FUNCTION outcome(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'ok';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE; END $$;

CREATE SCHEMA md;
SET search_path = md, public;
CREATE TABLE tb_parent (pk_parent int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text, city text, zip text);
CREATE TABLE tb_child (pk_child int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_parent int NOT NULL REFERENCES tb_parent, note text);
INSERT INTO tb_parent VALUES (1, DEFAULT, 'p1', 'c1', 'z1'), (2, DEFAULT, 'p2', 'c2', 'z2');
INSERT INTO tb_child SELECT g, gen_random_uuid(), 1 + (g - 1) / 3, 'n' || g FROM generate_series(1, 6) g;
SELECT tviews.pg_tviews_create('md.tv_parent', $$
    SELECT pk_parent, id, jsonb_build_object('name', name, 'city', city, 'zip', zip) AS data
    FROM md.tb_parent $$);
SELECT tviews.pg_tviews_create('md.tv_child', $$
    SELECT c.pk_child, c.id, c.fk_parent,
           jsonb_build_object('note', c.note, 'parent', p.data) AS data
    FROM md.tb_child c JOIN md.tv_parent p ON p.pk_parent = c.fk_parent $$);
CREATE VIEW v_parent_over AS SELECT pk_parent, data FROM tv_parent;

-- TVIEW rows written since the last call (stats are reset by each call).
CREATE FUNCTION written() RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE n bigint;
BEGIN
    SELECT coalesce(sum(rows_written), 0) INTO n FROM tviews.stats;
    PERFORM tviews.pg_tviews_reset_stats_all();
    RETURN n;
END $$;

CREATE FUNCTION fresh_all(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('md.tv_parent', 'pk_parent', label);
    PERFORM assert_fresh('md.tv_child', 'pk_child', label);
END $$;

-- §1 Three single-field UPDATEs of one parent, then its document read back: the
-- parent and its three children are written once (4 rows, not 12), and the
-- document returned holds all three changes.
CREATE FUNCTION rename3(p int, v text) RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    UPDATE md.tb_parent SET city = v WHERE pk_parent = p;
    UPDATE md.tb_parent SET zip = v WHERE pk_parent = p;
    RETURN (SELECT data FROM md.tv_parent WHERE pk_parent = p);
END $$;
SELECT written();
SELECT must(rename3(1, 'a') = '{"name": "a", "city": "a", "zip": "a"}', '§1 the read inside the function is stale');
SELECT must(written() = 4, '§1 rows written: not once per mutation');
SELECT fresh_all('§1');

-- §2 The same without a read: refreshed once, when the outer statement ends.
CREATE FUNCTION touch3(p int, v text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    UPDATE md.tb_parent SET city = v WHERE pk_parent = p;
    UPDATE md.tb_child SET note = v WHERE fk_parent = p;
END $$;
SELECT written();
SELECT touch3(2, 'b');
SELECT must(written() = 4, '§2 rows written: not once per mutation');
SELECT fresh_all('§2');

-- §3 A read between two writes flushes what was queued before it: the second
-- write refreshes again at the end.
CREATE FUNCTION write_read_write(p int) RETURNS text LANGUAGE plpgsql AS $$
DECLARE seen text;
BEGIN
    UPDATE md.tb_parent SET name = 'mid' WHERE pk_parent = p;
    SELECT data->>'name' INTO seen FROM md.tv_parent WHERE pk_parent = p;
    UPDATE md.tb_parent SET name = 'end' WHERE pk_parent = p;
    RETURN seen;
END $$;
SELECT written();
SELECT must(write_read_write(1) = 'mid', '§3 the read between the writes is stale');
SELECT must(written() = 8, '§3 rows written: one flush at the read, one at the end');
SELECT fresh_all('§3');

-- §4 A read through a view over the TVIEW's table.
CREATE FUNCTION via_view(p int, v text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    RETURN (SELECT data->>'name' FROM md.v_parent_over WHERE pk_parent = p);
END $$;
SELECT must(via_view(1, 'v4') = 'v4', '§4 a read through a view is stale');

-- §5 A read inside a STABLE function, PL/pgSQL and SQL.
CREATE FUNCTION doc_plpgsql(p int) RETURNS jsonb STABLE LANGUAGE plpgsql AS $$
BEGIN RETURN (SELECT data FROM md.tv_parent WHERE pk_parent = p); END $$;
CREATE FUNCTION doc_sql(p int) RETURNS jsonb STABLE LANGUAGE sql AS $$
    SELECT data FROM md.tv_parent WHERE pk_parent = p $$;
CREATE FUNCTION via_stable(p int, v text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    RETURN (doc_plpgsql(p)->>'name') || '/' || (doc_sql(p)->>'name');
END $$;
SELECT must(via_stable(1, 'v5') = 'v5/v5', '§5 a read in a STABLE function is stale');

-- §6 A SQL-language mutation.
CREATE FUNCTION sql_mutation(p int, v text) RETURNS text LANGUAGE sql AS $$
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    UPDATE md.tb_parent SET city = v WHERE pk_parent = p;
    SELECT data->>'name' || '/' || (data->>'city') FROM md.tv_parent WHERE pk_parent = p $$;
SELECT written();
SELECT must(sql_mutation(1, 'v6') = 'v6/v6', '§6 a SQL function''s read is stale');
SELECT must(written() = 4, '§6 rows written: not once per mutation');

-- §7 A cursor loop and RETURN QUERY over the embedding TVIEW.
CREATE FUNCTION via_cursor(p int, v text) RETURNS int LANGUAGE plpgsql AS $$
DECLARE r record; n int := 0;
BEGIN
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    FOR r IN SELECT data FROM md.tv_child WHERE fk_parent = p LOOP
        IF r.data->'parent'->>'name' = v THEN n := n + 1; END IF;
    END LOOP;
    RETURN n;
END $$;
SELECT must(via_cursor(1, 'v7') = 3, '§7 a cursor loop reads stale children');
CREATE FUNCTION via_return_query(p int, v text) RETURNS SETOF text LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    RETURN QUERY SELECT data->'parent'->>'name' FROM md.tv_child WHERE fk_parent = p;
END $$;
SELECT must(bool_and(n = 'v8') AND count(*) = 3, '§7 RETURN QUERY reads stale children')
FROM via_return_query(1, 'v8') n;
SELECT fresh_all('§7');

-- §8 A subtransaction rolled back inside the mutation takes its queued work with
-- it; the rest is refreshed.
CREATE FUNCTION with_handler(p int) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_parent SET city = 'kept' WHERE pk_parent = p;
    BEGIN
        UPDATE md.tb_parent SET name = 'rolled back' WHERE pk_parent = p;
        UPDATE md.tb_child SET note = 'rolled back' WHERE fk_parent = p;
        RAISE EXCEPTION 'undo';
    EXCEPTION WHEN raise_exception THEN NULL;
    END;
END $$;
SELECT with_handler(2);
SELECT must(data->>'city' = 'kept' AND data->>'name' <> 'rolled back', '§8 ' || data::text)
FROM tv_parent WHERE pk_parent = 2;
SELECT fresh_all('§8');

-- §9 A refresh that fails raises at the read inside the function, where a
-- handler sees it; without a read, when the outer statement ends. The work stays
-- queued after a handler: the outer statement fails with 54000 either way.
SET pg_tviews.max_refresh_rows = 2;
CREATE FUNCTION over_limit(p int, v text, read_back boolean) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    IF read_back THEN
        BEGIN
            PERFORM 1 FROM md.tv_child WHERE fk_parent = p;
        EXCEPTION WHEN program_limit_exceeded THEN
            RAISE EXCEPTION 'caught at the read' USING ERRCODE = 'P0042';
        END;
    END IF;
    RETURN 'returned';
END $$;
-- Each call is the outermost statement of a DO block's PERFORM: its handler sees
-- what the call raised. (Called from a function, the flush would run after it.)
CREATE TEMP TABLE seen (what text);
DO $$ BEGIN PERFORM md.over_limit(1, 'v9', true);
EXCEPTION WHEN OTHERS THEN INSERT INTO seen VALUES ('read ' || SQLSTATE); END $$;
DO $$ BEGIN PERFORM md.over_limit(1, 'v9b', false);
EXCEPTION WHEN OTHERS THEN INSERT INTO seen VALUES ('end ' || SQLSTATE); END $$;
SELECT must(string_agg(what, ',' ORDER BY what) = 'end 54000,read P0042', '§9 ' || string_agg(what, ','))
FROM seen;
RESET pg_tviews.max_refresh_rows;
SELECT fresh_all('§9');

-- §10 Inside an explicit transaction, a top-level read after the mutation.
BEGIN;
SELECT touch3(1, 'v10');
SELECT must(data->'parent'->>'name' = 'v10', '§10 ' || data::text) FROM tv_child WHERE pk_child = 1;
COMMIT;
SELECT fresh_all('§10');

-- §11 A writing outer statement whose function writes: one flush at its end.
CREATE FUNCTION note_children(p int, v text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_child SET note = v WHERE fk_parent = p;
    RETURN v;
END $$;
SELECT written();
UPDATE tb_parent SET zip = note_children(pk_parent, 'v11') WHERE pk_parent = 2;
SELECT must(written() = 4, '§11 rows written: not once per outer statement');
SELECT fresh_all('§11');

-- §12 A write to a table no TVIEW reads, whose trigger writes one a TVIEW does:
-- refreshed when that statement ends (autocommit commits, nothing left queued).
CREATE TABLE tb_log (id int);
CREATE FUNCTION log_touches_child() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_child SET note = 'via log ' || NEW.id WHERE pk_child = 1;
    RETURN NULL;
END $$;
CREATE TRIGGER log_touches_child AFTER INSERT ON tb_log
    FOR EACH ROW EXECUTE FUNCTION log_touches_child();
INSERT INTO tb_log VALUES (12);
SELECT must(data->>'note' = 'via log 12', '§12 ' || data::text) FROM tv_child WHERE pk_child = 1;
SELECT fresh_all('§12');

-- §13 A statement whose STABLE function reads a TVIEW while the mutation's work
-- is queued: the function's query reads on a snapshot taken after the refresh,
-- and the statement's own scan keeps its snapshot (its rows are updated once).
CREATE FUNCTION child_count(p int) RETURNS bigint STABLE LANGUAGE plpgsql AS $$
BEGIN RETURN (SELECT count(*) FROM md.tv_child WHERE fk_parent = p); END $$;
INSERT INTO tb_parent SELECT g, gen_random_uuid(), 'p' || g, 'c', 'z' FROM generate_series(3, 400) g;
CREATE FUNCTION count_into_zip() RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE n bigint;
BEGIN
    INSERT INTO md.tb_child VALUES (100, DEFAULT, 1, 'new'), (101, DEFAULT, 2, 'new');
    UPDATE md.tb_parent SET zip = child_count(pk_parent)::text;
    GET DIAGNOSTICS n = ROW_COUNT;
    RETURN n;
END $$;
SELECT must(count_into_zip() = 400, '§13 rows updated');
SELECT must(string_agg(pk_parent || '=' || zip, ',' ORDER BY pk_parent) = '1=4,2=4,3=0',
            '§13 ' || string_agg(pk_parent || '=' || zip, ',' ORDER BY pk_parent))
FROM tb_parent WHERE pk_parent <= 3;
SELECT fresh_all('§13');

-- §14 A read refreshes inside a subtransaction that then rolls back: the work it
-- refreshed is queued again, and still refreshed when the statement ends.
CREATE FUNCTION read_then_undo(p int, v text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE md.tb_parent SET name = v WHERE pk_parent = p;
    BEGIN
        PERFORM count(*) FROM md.tv_parent;
        RAISE EXCEPTION 'undo';
    EXCEPTION WHEN raise_exception THEN NULL;
    END;
END $$;
SELECT read_then_undo(1, 'v14');
SELECT must(data->'parent'->>'name' = 'v14', '§14 ' || data::text) FROM tv_child WHERE pk_child = 1;
SELECT fresh_all('§14');

DROP SCHEMA md CASCADE;
DROP EXTENSION pg_tviews CASCADE;

\echo 'mutation deferral: PASS'
