-- A TVIEW that reads only other TVIEWs' tables is refreshed with them.
--
-- A read of another TVIEW's table that is not an embed is mapped like a base
-- table (ADR 0157, reads of another TVIEW's table): the inner TVIEW's table gets
-- the delta triggers, which fire on the flush's own writes. A definition that
-- reads no base table at all still gets them: it is never left without a
-- trigger behind a warning.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_reads_only_tviews.sql
-- expect-output: reads only TVIEWs: PASS
-- reject-output: No base table dependencies

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'alan');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$$);

-- Its rows are tv_user's rows.
SELECT pg_tviews_create('tv_badge', $$
    SELECT pk_user AS pk_badge, id, jsonb_build_object('holder', data) AS data FROM tv_user
$$);
-- One row over all of tv_user: no write maps to its key, so it declares a
-- full refresh.
SELECT pg_tviews_create_or_replace('tv_roster', $$
    SELECT 1::bigint AS pk_roster, '00000000-0000-0000-0000-000000000001'::uuid AS id,
           jsonb_build_object('names', jsonb_agg(data->>'name' ORDER BY pk_user)) AS data
    FROM tv_user
$$, options => '{"uncascaded_policy": "full_refresh"}');

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_badge t FULL JOIN tviews.public__tv_badge v USING (pk_badge)
               WHERE t.pk_badge IS NULL OR v.pk_badge IS NULL OR t.data IS DISTINCT FROM v.data)
    THEN
        RAISE EXCEPTION 'FAIL (%): tv_badge diverges from its view', label;
    END IF;
    IF EXISTS (SELECT 1 FROM tv_roster t FULL JOIN tviews.public__tv_roster v USING (pk_roster)
               WHERE t.pk_roster IS NULL OR v.pk_roster IS NULL OR t.data IS DISTINCT FROM v.data)
    THEN
        RAISE EXCEPTION 'FAIL (%): tv_roster diverges from its view', label;
    END IF;
END $$;

UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
SELECT check_fresh('update');
INSERT INTO tb_user (pk_user, name) VALUES (3, 'barbara');
SELECT check_fresh('insert');
DELETE FROM tb_user WHERE pk_user = 2;
SELECT check_fresh('delete');

\echo 'reads only TVIEWs: PASS'
