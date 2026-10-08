-- A direct patch whose jsonb_delta is gone by flush time fails; it never calls a
-- same-named function from another schema.
--
-- Direct patches are captured while jsonb_delta is installed and applied when the
-- statement flushes. If jsonb_delta is dropped in between, the flush must not
-- fall back to public.jsonb_smart_patch_scalar: any role with CREATE on public
-- could have planted one, and the flush runs it as the TVIEW's owner.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_jsonb_delta_dropped_before_flush.sql
-- expect-output: jsonb_delta dropped before flush: refused

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE SCHEMA jd;
CREATE EXTENSION jsonb_delta SCHEMA jd;
CREATE EXTENSION pg_tviews;

DROP ROLE IF EXISTS regress_planter;
CREATE ROLE regress_planter;
GRANT CREATE ON SCHEMA public TO regress_planter;

-- A direct-patch TVIEW, and a second TVIEW whose base table carries the writing
-- statement, so the first one's flush waits for that statement to end.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, bio text);
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann', 'b');
SELECT pg_tviews_create('tv_user',
  $q$SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user$q$);
CREATE TABLE tb_log (pk_log int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), note text);
INSERT INTO tb_log VALUES (1, DEFAULT, 'n');
SELECT pg_tviews_create('tv_log',
  $q$SELECT pk_log, id, jsonb_build_object('note', note) AS data FROM tb_log$q$);

SET ROLE regress_planter;
CREATE FUNCTION public.jsonb_smart_patch_scalar(target jsonb, patch jsonb) RETURNS jsonb
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'PLANTED jsonb_smart_patch_scalar ran as %', current_user;
END $$;
RESET ROLE;

-- Captures a direct patch of tv_user, then drops jsonb_delta, inside one statement.
CREATE FUNCTION patch_then_drop() RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_user SET bio = 'patched' WHERE pk_user = 1;
    DROP EXTENSION jsonb_delta;
    RETURN 'logged';
END $$;

DO $$
BEGIN
    UPDATE tb_log SET note = patch_then_drop() WHERE pk_log = 1;
    RAISE EXCEPTION 'the flush applied a patch without jsonb_delta';
EXCEPTION WHEN OTHERS THEN
    IF SQLERRM LIKE '%PLANTED%' OR SQLERRM NOT LIKE '%jsonb_delta%' THEN
        RAISE EXCEPTION 'wrong outcome: %', SQLERRM;
    END IF;
END $$;

\echo 'jsonb_delta dropped before flush: refused'

DROP FUNCTION public.jsonb_smart_patch_scalar(jsonb, jsonb);
DROP OWNED BY regress_planter;
DROP ROLE regress_planter;
