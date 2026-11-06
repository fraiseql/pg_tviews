-- Who runs a TVIEW's code, and under which settings: shared by the privileges
-- tests. Run as a superuser, after CREATE EXTENSION.
--
--   \ir ../../lib/who.sql
--   SELECT regress_roles('regress_x');   -- regress_x_owner_a, _owner_b, _writer, _reader, _stranger
--   ... a definition calling public.folded('step') or public.who('step') ...
--   SELECT count(*) FROM public.who_log WHERE cu <> 'regress_x_owner_a';
--   SELECT regress_drop_roles('regress_x');
--
-- who(step) records the current user and the settings a value's rendering
-- depends on. folded(step) is IMMUTABLE with a constant argument, so the planner
-- evaluates it while planning a query that calls it: it records who planned it.

CREATE TABLE public.who_log (step text, cu name, settings jsonb);
GRANT INSERT, SELECT ON public.who_log TO PUBLIC;

CREATE FUNCTION public.who(step text) RETURNS int
LANGUAGE plpgsql VOLATILE AS $$
BEGIN
    INSERT INTO public.who_log VALUES (step, current_user, pg_catalog.jsonb_build_object(
        'DateStyle', current_setting('DateStyle'),
        'TimeZone', current_setting('TimeZone'),
        'IntervalStyle', current_setting('IntervalStyle'),
        'extra_float_digits', current_setting('extra_float_digits'),
        'lc_monetary', current_setting('lc_monetary'),
        'search_path', current_setting('search_path')));
    RETURN 1;
END $$;

CREATE FUNCTION public.folded(step text) RETURNS int
LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    RETURN public.who(step);
END $$;

-- prefix_owner_a and prefix_owner_b (LOGIN, CREATE on schema public),
-- prefix_writer, prefix_reader and prefix_stranger (LOGIN only): each test grants
-- what its scenario needs.
CREATE FUNCTION public.regress_roles(prefix text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    r text;
BEGIN
    PERFORM public.regress_drop_roles(prefix);
    FOREACH r IN ARRAY ARRAY['owner_a', 'owner_b', 'writer', 'reader', 'stranger'] LOOP
        EXECUTE format('CREATE ROLE %I LOGIN', prefix || '_' || r);
    END LOOP;
    EXECUTE format('GRANT CREATE ON SCHEMA public TO %I, %I',
                   prefix || '_owner_a', prefix || '_owner_b');
END $$;

CREATE FUNCTION public.regress_drop_roles(prefix text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    r text;
BEGIN
    FOREACH r IN ARRAY ARRAY['owner_a', 'owner_b', 'writer', 'reader', 'stranger'] LOOP
        IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = prefix || '_' || r) THEN
            EXECUTE format('DROP OWNED BY %I CASCADE', prefix || '_' || r);
            EXECUTE format('DROP ROLE %I', prefix || '_' || r);
        END IF;
    END LOOP;
END $$;
