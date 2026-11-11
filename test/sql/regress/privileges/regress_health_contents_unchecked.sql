-- pg_tviews_check_health()'s `contents` component is OK only when it checked
-- every TVIEW. A TVIEW it did not check is listed with the reason, under WARNING.
--
-- A TVIEW is checked for a caller that has its owner's privileges, or that may
-- read the whole TVIEW table (SELECT on it, USAGE on its schema, no row security
-- applying): "empty while its definition yields rows" tells such a caller only
-- what a correct table would show it anyway.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_health_contents_unchecked.sql
-- expect-output: contents_unchecked: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_hcu');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'contents_unchecked FAIL: %', what; END IF; END $$;
CREATE FUNCTION contents() RETURNS TABLE (status text, message text) LANGUAGE sql AS $$
    SELECT status, message FROM tviews.pg_tviews_check_health() WHERE component = 'contents' $$;
GRANT EXECUTE ON FUNCTION contents() TO PUBLIC;

SET ROLE regress_hcu_owner_a;
CREATE TABLE public.tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO public.tb_doc VALUES (1, DEFAULT, 'd1');
SELECT tviews.pg_tviews_create('public.tv_doc', $q$
    SELECT pk_doc, id, jsonb_build_object('id', id, 'title', title) AS data FROM public.tb_doc $q$);
RESET ROLE;

-- 1. A caller without the owner's privileges and without SELECT on tv_doc: the
--    TVIEW is not checked, so contents is not OK.
SET ROLE regress_hcu_stranger;
SELECT must(status = 'WARNING' AND message LIKE '%not checked: public.tv_doc (owner''s privileges required)%',
            'an unchecked TVIEW reported as: ' || status || ' ' || message)
FROM contents();
RESET ROLE;
-- The owner checks it, and it holds its rows.
SET ROLE regress_hcu_owner_a;
SELECT must(status = 'OK', 'the owner''s check: ' || message) FROM contents();
RESET ROLE;

-- 2. A whole-table reader is checked: SELECT on tv_doc, USAGE on public.
GRANT SELECT ON public.tv_doc TO regress_hcu_reader;
SET ROLE regress_hcu_reader;
SELECT must(status = 'OK', 'a whole-table reader on a filled TVIEW: ' || message) FROM contents();
RESET ROLE;
TRUNCATE public.tv_doc;
SET ROLE regress_hcu_reader;
SELECT must(status = 'WARNING' AND message LIKE 'empty while their definition yields rows: public.tv_doc%'
            AND message NOT LIKE '%not checked%',
            'a whole-table reader on an emptied TVIEW: ' || message)
FROM contents();
RESET ROLE;
SET ROLE regress_hcu_stranger;
SELECT must(message NOT LIKE '%empty while%' AND message LIKE '%not checked: public.tv_doc%',
            'a stranger learned of the emptied TVIEW: ' || message)
FROM contents();
RESET ROLE;

-- 3. Row security applying to the reader: it may not read the whole table, so
--    the TVIEW is not checked for it.
ALTER TABLE public.tv_doc ENABLE ROW LEVEL SECURITY;
CREATE POLICY doc_reader ON public.tv_doc FOR SELECT TO regress_hcu_reader USING (true);
SET ROLE regress_hcu_reader;
SELECT must(status = 'WARNING' AND message LIKE '%not checked: public.tv_doc (owner''s privileges required)%'
            AND message NOT LIKE '%empty while%',
            'a reader under row security was checked: ' || message)
FROM contents();
RESET ROLE;
DROP POLICY doc_reader ON public.tv_doc;
ALTER TABLE public.tv_doc DISABLE ROW LEVEL SECURITY;

-- 4. A member of the owner's role is checked whatever its grants.
GRANT regress_hcu_owner_a TO regress_hcu_writer;
SET ROLE regress_hcu_writer;
SELECT must(status = 'WARNING' AND message LIKE 'empty while their definition yields rows: public.tv_doc%',
            'a member of the owner on an emptied TVIEW: ' || message)
FROM contents();
RESET ROLE;

SELECT 'contents_unchecked: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_doc;
DROP FUNCTION contents();
SELECT regress_drop_roles('regress_hcu');
