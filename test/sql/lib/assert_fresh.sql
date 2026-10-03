-- Freshness checks shared by the regression and differential suites: a TVIEW is
-- fresh when its table holds exactly the rows of its backing view, compared over
-- the view's columns (the table adds created_at / updated_at).
--
--   \ir lib/assert_fresh.sql
--   SELECT assert_fresh('tv_order', 'pk_order', 'an UPDATE of tb_line');

-- NULL when tv_<entity> and v_<entity> agree; otherwise how they differ, with the
-- `key` values of the offending rows.
CREATE OR REPLACE FUNCTION fresh_diff(tv regclass, key text) RETURNS text
LANGUAGE plpgsql AS $$
DECLARE
    v regclass;
    cols text;
    tv_only bigint; v_only bigint; tv_keys text; v_keys text;
    q text := 'SELECT pg_catalog.count(*), pg_catalog.string_agg(k, '','' ORDER BY k) '
              'FROM (SELECT %1$s::text AS k FROM (SELECT %2$s FROM %3$s EXCEPT ALL SELECT %2$s FROM %4$s) x) y';
BEGIN
    SELECT pg_catalog.to_regclass(pg_catalog.quote_ident(n.nspname) || '.'
                                  || pg_catalog.quote_ident('v_' || pg_catalog.substr(c.relname, 4)))
      INTO v
      FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
     WHERE c.oid = tv;
    SELECT pg_catalog.string_agg(pg_catalog.quote_ident(attname), ', ' ORDER BY attnum) INTO cols
    FROM pg_catalog.pg_attribute WHERE attrelid = v AND attnum > 0 AND NOT attisdropped;
    EXECUTE format(q, pg_catalog.quote_ident(key), cols, tv, v) INTO tv_only, tv_keys;
    EXECUTE format(q, pg_catalog.quote_ident(key), cols, v, tv) INTO v_only, v_keys;
    IF tv_only + v_only = 0 THEN
        RETURN NULL;
    END IF;
    RETURN format('%s: %s row(s) only in the table [%s], %s only in the view [%s]',
                  tv, tv_only, coalesce(tv_keys, ''), v_only, coalesce(v_keys, ''));
END $$;

-- Raise unless the TVIEW is fresh; `label` says after what.
CREATE OR REPLACE FUNCTION assert_fresh(tv regclass, key text, label text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE diff text := fresh_diff(tv, key);
BEGIN
    IF diff IS NOT NULL THEN
        RAISE EXCEPTION 'STALE after %: %', label, diff;
    END IF;
END $$;
