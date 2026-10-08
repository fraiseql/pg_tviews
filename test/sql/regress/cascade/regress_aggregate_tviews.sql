-- Regression test for issue #58: aggregate / summary TVIEWs.
--
-- An entity with no tb_<entity> whose rows are the GROUP BY groups of its source
-- tables was rejected at create time (#49). pg_tviews_create_aggregate() creates
-- one, keyed by pk_<entity>, and keeps each group in sync: a new group is inserted,
-- a changed one recomputed, an emptied one deleted, and a row that moves between
-- groups refreshes both.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_aggregate_tviews.sql
-- expect-output: aggregate_tviews: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
CREATE TABLE tb_order (
    pk_order BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id       UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user  BIGINT NOT NULL REFERENCES tb_user(pk_user),
    total    NUMERIC NOT NULL
);
INSERT INTO tb_user (name) SELECT 'u' || g FROM generate_series(1, 10) g;
INSERT INTO tb_order (fk_user, total) VALUES (1, 10), (1, 20), (2, 5);

CREATE FUNCTION must(ok BOOLEAN, msg TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION '#58 FAIL: %', msg; END IF; END $$;
CREATE FUNCTION in_sync() RETURNS BOOLEAN LANGUAGE plpgsql AS $$
BEGIN
    RETURN (SELECT count(*) = 0 FROM tviews.public__tv_user_summary v
            FULL JOIN tv_user_summary t USING (pk_user_summary)
            WHERE t.data IS DISTINCT FROM v.data OR t.id IS DISTINCT FROM v.id);
END $$;
CREATE FUNCTION rejects(sql TEXT, keys JSONB, pattern TEXT) RETURNS BOOLEAN LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_tviews_create_aggregate('tv_bad_summary', sql, keys);
    RETURN false;
EXCEPTION WHEN OTHERS THEN
    RETURN SQLERRM LIKE pattern;
END $$;

-- ========================================================================
-- Cycle 1: creation and the first groups
-- ========================================================================
SELECT pg_tviews_create_aggregate('tv_user_summary', $$
    SELECT o.fk_user AS pk_user_summary, u.id,
           jsonb_build_object('name', u.name, 'orders', count(*), 'total', sum(o.total)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id, u.name
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');

SELECT must((SELECT count(*) FROM tv_user_summary) = 2, 'two groups after creation');
SELECT must((SELECT data->>'total' FROM tv_user_summary WHERE pk_user_summary = 1) = '30',
            'initial total of user 1');

-- ========================================================================
-- Cycle 2: insert (new group), update, move between groups, empty a group
-- ========================================================================
INSERT INTO tb_order (fk_user, total) VALUES (3, 7);
SELECT must(in_sync(), 'after a new group');
UPDATE tb_order SET total = 25 WHERE pk_order = 2;
SELECT must(in_sync(), 'after an update inside a group');
UPDATE tb_order SET fk_user = 2 WHERE pk_order = 1;
SELECT must(in_sync(), 'after moving an order from user 1 to user 2');
DELETE FROM tb_order WHERE fk_user = 3;
SELECT must(in_sync() AND NOT EXISTS (SELECT 1 FROM tv_user_summary WHERE pk_user_summary = 3),
            'after emptying a group');
UPDATE tb_user SET name = 'renamed' WHERE pk_user = 2;
SELECT must(in_sync(), 'after a change to the joined source tb_user');

-- ========================================================================
-- Cycle 3: definitions that cannot be maintained per group are rejected
-- ========================================================================
SELECT must(rejects($$ SELECT o.fk_user AS pk_bad_summary, u.id,
                     jsonb_build_object('r', rank() OVER (ORDER BY o.fk_user)) AS data
                     FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
                     GROUP BY o.fk_user, u.id $$,
                    '{"tb_order": "fk_user"}', '%window functions%'), 'window function');
SELECT must(rejects($$ SELECT o.fk_user + 0 AS pk_bad_summary, u.id,
                     jsonb_build_object('n', count(*)) AS data
                     FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
                     GROUP BY o.fk_user, u.id $$,
                    '{"tb_order": "fk_user"}', '%plain column%'), 'expression key');
SELECT must(rejects($$ SELECT o.fk_user AS pk_bad_summary, max(u.id::text) AS id,
                     jsonb_build_object('n', count(*)) AS data
                     FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
                     GROUP BY o.fk_user + 0 $$,
                    '{"tb_order": "fk_user"}', '%GROUP BY%'), 'key missing from GROUP BY');
SELECT must(rejects($$ SELECT o.fk_user AS pk_bad_summary, u.id,
                     jsonb_build_object('n', count(*)) AS data
                     FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
                     GROUP BY o.fk_user, u.id $$,
                    '{"tb_invoice": "fk_user"}', '%not a table the definition reads%'),
            'unknown source table');
SELECT must(rejects($$ SELECT o.fk_user AS pk_bad_summary, u.id,
                     jsonb_build_object('n', count(*)) AS data
                     FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
                     GROUP BY o.fk_user, u.id $$,
                    '{"tb_order": "fk_customer"}', '%has no column%'), 'unknown column');
SELECT must(to_regclass('tv_bad_summary') IS NULL
            AND NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'bad_summary'),
            'a rejected aggregate left objects behind');

-- ========================================================================
-- Cycle 4: differential check under a seeded random workload
-- ========================================================================
SELECT setseed(0.58);
DO $$
DECLARE
    op INT;
BEGIN
    FOR i IN 1..300 LOOP
        op := floor(random() * 4)::int;
        IF op = 0 THEN
            INSERT INTO tb_order (fk_user, total)
            VALUES (1 + floor(random() * 10)::int, round((random() * 100)::numeric, 2));
        ELSIF op = 1 THEN
            UPDATE tb_order SET total = round((random() * 100)::numeric, 2)
            WHERE pk_order = (SELECT pk_order FROM tb_order ORDER BY random() LIMIT 1);
        ELSIF op = 2 THEN
            UPDATE tb_order SET fk_user = 1 + floor(random() * 10)::int
            WHERE pk_order = (SELECT pk_order FROM tb_order ORDER BY random() LIMIT 1);
        ELSE
            DELETE FROM tb_order
            WHERE pk_order = (SELECT pk_order FROM tb_order ORDER BY random() LIMIT 1);
        END IF;
    END LOOP;
END $$;
SELECT must(in_sync(), 'differential check after 300 random operations');

-- ========================================================================
-- Cycle 5: renaming a group key column keeps the aggregate maintained
-- ========================================================================
ALTER TABLE tb_order RENAME COLUMN fk_user TO fk_customer;
SELECT must((SELECT group_keys->>'tb_order' FROM pg_tview_meta WHERE entity = 'user_summary')
            = 'fk_customer', 'group_keys not updated by the rename');
UPDATE tb_order SET fk_customer = 4 WHERE pk_order = (SELECT min(pk_order) FROM tb_order);
SELECT must(in_sync(), 'after a move once the group key column was renamed');

\echo 'aggregate_tviews: PASS'
