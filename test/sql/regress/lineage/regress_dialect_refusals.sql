-- Two evaluations of a TVIEW's definition over the same rows return the same
-- rows (dialect.md, Axis C and D). A definition that reads what changes with no
-- write, or what no frame can pin, is refused with 0A000 at any level of its view
-- chain: a system column other than xmin and tableoid, a volatile function (the
-- operators and casts it backs included), and a built-in that reads the session,
-- the transaction or the server instead of the rows.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_dialect_refusals.sql
--
-- expect-output: dialect refusals: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- Raise unless `stmt` fails with SQLSTATE `want` and a message matching `like`.
CREATE FUNCTION refused(stmt text, want text, like_ text, what text) RETURNS void
LANGUAGE plpgsql AS $f$
DECLARE state text; msg text;
BEGIN
    BEGIN
        EXECUTE stmt;
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, msg = MESSAGE_TEXT;
        IF state <> want OR msg NOT LIKE like_ THEN
            RAISE EXCEPTION 'FAIL: % failed with % (want %): %', what, state, want, msg;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: % was accepted', what;
END $f$;

-- `refused` for the TVIEW tv_doc defined by `data` (its data expression) and `tail`.
CREATE FUNCTION refused_doc(data text, tail text, like_ text, what text,
                            options text DEFAULT '{}') RETURNS void
LANGUAGE sql AS $f$
    SELECT refused(format($s$SELECT tviews.pg_tviews_create_or_replace('tv_doc', %L, %L)$s$,
                          format('SELECT d.pk_doc, d.id, %s AS data FROM tb_doc d %s', data, tail),
                          options),
                   '0A000', like_, what)
$f$;

CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     title text, n int);
INSERT INTO tb_doc (pk_doc, title, n) VALUES (1, 'd1', 1), (2, 'd2', 2);

-- System columns that change with no write pg_tviews sees.
SELECT refused_doc($$jsonb_build_object('at', d.ctid::text)$$, '', '%system column ctid%', 'ctid');
SELECT refused_doc($$jsonb_build_object('t', d.title)$$, 'WHERE d.xmax = 0', '%system column xmax%',
                   'xmax in a filter');
SELECT refused_doc($$jsonb_build_object('c', d.cmin::text)$$, '', '%system column cmin%', 'cmin');
SELECT refused_doc($$jsonb_build_object('c', d.cmax::text)$$, '', '%system column cmax%', 'cmax');

-- Volatile functions, wherever the output depends on them.
SELECT refused_doc($$jsonb_build_object('r', random())$$, '', '%random()%volatile%', 'random()');
SELECT refused_doc($$jsonb_build_object('t', d.title)$$, 'WHERE random() < 2', '%random()%volatile%',
                   'random() in a filter');
SELECT refused($s$SELECT tviews.pg_tviews_create_or_replace('tv_doc',
    'SELECT d.pk_doc, gen_random_uuid() AS id, jsonb_build_object(''t'', d.title) AS data FROM tb_doc d')$s$,
    '0A000', '%gen_random_uuid()%volatile%', 'gen_random_uuid() as the id');
SELECT refused_doc($$jsonb_build_object('n', (SELECT max(e.n) FROM tb_doc e WHERE e.n < random() * 10))$$,
                   '', '%random()%volatile%', 'random() in a subquery');
SELECT refused_doc($$jsonb_build_object('t', clock_timestamp())$$, '', '%clock_timestamp()%volatile%',
                   'clock_timestamp() under time_refresh external', '{"time_refresh": "external"}');
-- A user function marked VOLATILE (the default), declared in function_reads.
CREATE FUNCTION label(t text) RETURNS text LANGUAGE sql AS $$ SELECT upper(t) $$;
SELECT refused_doc($$jsonb_build_object('t', public.label(d.title))$$, '', '%public.label(text)%volatile%',
                   'a volatile user function', '{"function_reads": {"public.label(text)": []}}');
-- An operator a volatile function backs.
CREATE FUNCTION jitter(a int, b int) RETURNS int LANGUAGE sql VOLATILE AS $$ SELECT a + b $$;
CREATE OPERATOR +~ (LEFTARG = int, RIGHTARG = int, FUNCTION = jitter);
SELECT refused_doc($$jsonb_build_object('n', d.n +~ 1)$$, '', '%public.jitter(integer,integer)%volatile%',
                   'an operator backed by a volatile function',
                   '{"function_reads": {"public.jitter(integer,integer)": []}}');

-- Built-ins that read the session, the transaction or the server.
SELECT refused_doc($$jsonb_build_object('p', pg_backend_pid())$$, '', '%pg_backend_pid()%', 'pg_backend_pid()');
SELECT refused_doc($$jsonb_build_object('a', inet_client_addr())$$, '', '%inet_client_addr()%',
                   'inet_client_addr()');
SELECT refused_doc($$jsonb_build_object('x', txid_current())$$, '', '%txid_current()%', 'txid_current()');
SELECT refused_doc($$jsonb_build_object('x', pg_current_xact_id())$$, '', '%pg_current_xact_id()%',
                   'pg_current_xact_id()');
SELECT refused_doc($$jsonb_build_object('t', pg_postmaster_start_time())$$, '',
                   '%pg_postmaster_start_time()%', 'pg_postmaster_start_time()');
SELECT refused_doc($$jsonb_build_object('t', d.title)$$,
                   $$WHERE has_table_privilege('tb_doc', 'SELECT')$$, '%has_table_privilege()%',
                   'has_table_privilege() in a filter');
SELECT refused_doc($$jsonb_build_object('u', SESSION_USER)$$, '', '%reads SESSION_USER%', 'SESSION_USER');
SELECT refused_doc($$jsonb_build_object('u', SYSTEM_USER)$$, '', '%system_user()%', 'SYSTEM_USER');
SELECT refused_doc($$jsonb_build_object('t', d.title)$$, 'TABLESAMPLE BERNOULLI (50)', '%TABLESAMPLE%',
                   'TABLESAMPLE');

-- In a view the definition reads.
CREATE VIEW v_doc_pid AS SELECT d.*, pg_backend_pid() AS pid FROM tb_doc d;
SELECT refused($s$SELECT tviews.pg_tviews_create('tv_doc',
    'SELECT d.pk_doc, d.id, jsonb_build_object(''p'', d.pid) AS data FROM v_doc_pid d')$s$,
    '0A000', '%pg_backend_pid()%', 'pg_backend_pid() in a view read');
-- So does CREATE TABLE tv_* AS.
SELECT refused($s$CREATE TABLE tv_doc AS
    SELECT d.pk_doc, d.id, jsonb_build_object('r', random()) AS data FROM tb_doc d$s$,
    '0A000', '%random()%volatile%', 'random() under CREATE TABLE AS');
DO $$ BEGIN
    IF to_regclass('tv_doc') IS NOT NULL OR EXISTS (SELECT 1 FROM tviews.pg_tview_meta) THEN
        RAISE EXCEPTION 'FAIL: a refused definition left a TVIEW behind';
    END IF;
END $$;

-- Accepted: xmin and tableoid (no column skip, no patch), immutable and stable
-- built-ins that read pinned settings, the database's name.
SELECT tviews.pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id,
           jsonb_build_object('v', d.xmin::text, 'rel', d.tableoid::regclass::text,
                              't', upper(d.title), 'y', to_char(DATE '2026-01-01', 'YYYY'),
                              'db', current_database(), 'g', d.n::numeric::text) AS data
    FROM tb_doc d $$) IS NOT NULL AS created;
UPDATE tb_doc SET title = 'd1b' WHERE pk_doc = 1;
SELECT CASE WHEN (SELECT data->>'t' FROM tv_doc WHERE pk_doc = 1) = 'D1B' THEN 'ok'
            ELSE 'FAIL: tv_doc not refreshed' END AS refreshed;

SELECT 'dialect refusals: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
