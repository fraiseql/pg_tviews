-- Shared by the row-security tests. Run as a superuser, after who.sql and
-- assert_fresh.sql.
--
--   \ir ../../lib/row_security.sql
--   SELECT refused('regress_x_owner_a', 'SELECT tviews.pg_tviews_create(...)', 'CREATE TVIEW',
--                  'regress_x_owner_a', 'public.tb_rec');
--
-- refused(role, statement, label, want_role, want_table) runs `statement` as
-- `role` (the session's role when NULL) in a subtransaction. It must fail
-- saying that row security applies on a table the TVIEW reads, naming
-- `want_role` and `want_table` (either may be NULL: not checked), and leaves
-- nothing behind.
CREATE FUNCTION public.refused(who text, stmt text, what text, want_role text, want_table text)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    BEGIN
        IF who IS NOT NULL THEN
            EXECUTE format('SET LOCAL ROLE %I', who);
        END IF;
        EXECUTE stmt;
    EXCEPTION WHEN OTHERS THEN
        IF SQLERRM NOT LIKE '%row security applies to role%'
           OR (want_role IS NOT NULL AND SQLERRM NOT LIKE '%role ' || want_role || ' %')
           OR (want_table IS NOT NULL AND SQLERRM NOT LIKE '%table ' || want_table || ',%') THEN
            RAISE EXCEPTION 'FAIL: % failed with another error: %', what, SQLERRM;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: % was accepted', what;
END $$;

-- tv_<e> against its backing view read as the TVIEW's owner.
CREATE FUNCTION public.owner_fresh(tv regclass, key text, label text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE diff text;
BEGIN
    EXECUTE format('SET LOCAL ROLE %I', (SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid = tv));
    diff := fresh_diff(tv, key);
    RESET ROLE;
    IF diff IS NOT NULL THEN
        RAISE EXCEPTION 'STALE after %: %', label, diff;
    END IF;
END $$;
GRANT EXECUTE ON FUNCTION fresh_diff(regclass, text) TO PUBLIC;
