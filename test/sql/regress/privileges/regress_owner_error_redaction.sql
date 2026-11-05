-- An error raised while a refresh runs as the TVIEW's owner reaches a caller
-- without the owner's privileges with its SQLSTATE and without the values the
-- owner read: no DETAIL, and a message naming the condition. The owner, and the
-- members of its role, get the error unchanged; the server log has it whole.
--
-- The refresh reads what the owner may read. A value it fails on (a cast of a
-- joined column, a row a constraint of the TVIEW's table refuses) would
-- otherwise reach the writer in the message or the DETAIL.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_owner_error_redaction.sql
-- expect-output: owner errors: values reach the owner's members only

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_er');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'owner errors: %', what; END IF; END $$;
-- What each role saw of each failed write.
CREATE TABLE public.seen (who name, probe text, state text, message text, detail text);
GRANT INSERT ON public.seen TO PUBLIC;
-- owner_a's TVIEWs: one whose table refuses a row without id, one casting a
-- joined column. The joined tables are owner_a's alone.
SET ROLE regress_er_owner_a;
CREATE TABLE public.tb_customer (pk_customer int PRIMARY KEY, email text);
INSERT INTO public.tb_customer VALUES (1, 'secret-email-value');
CREATE TABLE public.tb_order (pk_order int PRIMARY KEY, id uuid, fk_customer int, amount int);
INSERT INTO public.tb_order VALUES (1, gen_random_uuid(), 1, 1);
SELECT tviews.pg_tviews_create('public.tv_order', $q$
    SELECT o.pk_order, o.id, o.fk_customer,
           jsonb_build_object('amount', o.amount, 'email', c.email) AS data
    FROM public.tb_order o JOIN public.tb_customer c ON c.pk_customer = o.fk_customer $q$);
ALTER TABLE public.tv_order ALTER COLUMN id SET NOT NULL;
CREATE TABLE public.tb_vip (pk_vip int PRIMARY KEY, tier text);
INSERT INTO public.tb_vip VALUES (1, '5'), (2, 'secret-tier-value');
CREATE TABLE public.tb_ticket (pk_ticket int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                               fk_vip int);
SELECT tviews.pg_tviews_create('public.tv_ticket', $q$
    SELECT t.pk_ticket, t.id, t.fk_vip, jsonb_build_object('tier', v.tier::int) AS data
    FROM public.tb_ticket t JOIN public.tb_vip v ON v.pk_vip = t.fk_vip $q$);
GRANT INSERT ON public.tb_order, public.tb_ticket TO regress_er_writer, regress_er_reader;
RESET ROLE;
GRANT regress_er_owner_a TO regress_er_reader;

-- Each role makes both refreshes fail, in a DO block: the refresh runs when
-- the block's statement ends, inside the block's exception scope.
SELECT format($f$SET ROLE %1$I;
    DO $do$
    DECLARE
        s text; m text; d text;
    BEGIN
        INSERT INTO public.%2$I VALUES %3$s;
        RAISE EXCEPTION 'owner errors: %4$s did not fail';
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS s = RETURNED_SQLSTATE, m = MESSAGE_TEXT, d = PG_EXCEPTION_DETAIL;
        IF m LIKE 'owner errors:%%' THEN
            RAISE;
        END IF;
        INSERT INTO public.seen VALUES (current_user, %4$L, s, m, d);
    END $do$;
    RESET ROLE$f$, r, t, v, label)
FROM unnest(ARRAY['regress_er_writer', 'regress_er_reader', 'regress_er_owner_a']) r,
     (VALUES ('tb_order', '(2, NULL, 1, 1)', 'not null'),
             ('tb_ticket', '(1, gen_random_uuid(), 2)', 'cast')) x(t, v, label) \gexec

SELECT who, probe, state, message, detail FROM public.seen ORDER BY 1, 2;

-- The writer: the SQLSTATE, no value.
SELECT must(state = CASE probe WHEN 'not null' THEN '23502' ELSE '22P02' END,
            format('the writer got %s for %s', state, probe))
FROM public.seen WHERE who = 'regress_er_writer';
SELECT must(concat(message, detail) NOT LIKE '%secret%',
            format('the writer read a value through %s: %s %s', probe, message, detail))
FROM public.seen WHERE who = 'regress_er_writer';
SELECT must(count(*) = 2, 'the writer''s probes') FROM public.seen WHERE who = 'regress_er_writer';
-- The owner and a member of its role: the error as raised.
SELECT must(concat(message, detail) LIKE '%secret%',
            format('%s did not get the whole error for %s', who, probe))
FROM public.seen WHERE who IN ('regress_er_reader', 'regress_er_owner_a');
SELECT must(count(*) = 4, 'the owner''s probes')
FROM public.seen WHERE who IN ('regress_er_reader', 'regress_er_owner_a');

SELECT 'owner errors: values reach the owner''s members only' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.seen, public.tb_customer, public.tb_order, public.tb_vip, public.tb_ticket;
SELECT regress_drop_roles('regress_er');
