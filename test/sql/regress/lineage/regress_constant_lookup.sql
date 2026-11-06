-- Issue #284: a table read once by a constant predicate no longer refreshes the
-- whole TVIEW on every write to it.
--
-- tv_order reads tb_category twice: through its lines (linked to the order) and as
-- `(SELECT c.name FROM tb_category c WHERE c.name = 'default')`, linked to no
-- order. That read made the table all_keys, and under uncascaded_policy
-- full_refresh every write to tb_category refreshed every order, the linked reads'
-- mapping unused. Now a read with no link to the key, filtered by equalities, IN
-- lists or IS NULL of the table's columns against constants, keeps that filter: a
-- write refreshes the TVIEW in full only when an old or new row matches it, and
-- otherwise maps the rows the linked reads reach. Any other filter keeps the
-- table all_keys for every write.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_constant_lookup.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#284 FAIL: %', what; END IF; END $$;

CREATE TABLE tb_category (pk_category bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                          id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                          name text NOT NULL, kind text, retired_at timestamptz);
CREATE TABLE tb_order (pk_order bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                       id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_order_line (pk_order_line bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                            fk_order bigint NOT NULL REFERENCES tb_order,
                            fk_category bigint REFERENCES tb_category);
CREATE INDEX ON tb_order_line (fk_order);
CREATE INDEX ON tb_order_line (fk_category);
INSERT INTO tb_category (name, kind) SELECT 'cat' || g, CASE WHEN g % 2 = 0 THEN 'even' END
    FROM generate_series(1, 20) g;
INSERT INTO tb_category (name) VALUES ('default');
INSERT INTO tb_order (name) SELECT 'o' || g FROM generate_series(1, 200) g;
INSERT INTO tb_order_line (fk_order, fk_category)
    SELECT 1 + g % 200, 1 + g % 20 FROM generate_series(1, 600) g;

-- The linked read, the constant lookup.
SELECT tviews.pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id, jsonb_build_object('id', o.id,
        'lines', (SELECT jsonb_agg(c.name ORDER BY l.pk_order_line)
                  FROM tb_order_line l JOIN tb_category c ON c.pk_category = l.fk_category
                  WHERE l.fk_order = o.pk_order),
        'default_category', (SELECT c.name FROM tb_category c WHERE c.name = 'default')) AS data
    FROM tb_order o $$, '{"uncascaded_policy": "full_refresh"}');
-- ANDed equality and IN list, and IS NULL.
SELECT tviews.pg_tviews_create('tv_order_counts', $$
    SELECT o.pk_order AS pk_order_counts, o.id, jsonb_build_object('id', o.id,
        'picked', (SELECT count(*) FROM tb_category c
                   WHERE c.kind = 'even' AND c.name IN ('cat2', 'cat4')),
        'live', (SELECT count(*) FROM tb_category c WHERE c.retired_at IS NULL AND c.kind IS NULL))
        AS data
    FROM tb_order o $$, '{"uncascaded_policy": "full_refresh"}');
-- A filter that is no constant comparison: every write refreshes in full.
SELECT tviews.pg_tviews_create('tv_order_like', $$
    SELECT o.pk_order AS pk_order_like, o.id, jsonb_build_object('id', o.id,
        'd', (SELECT count(*) FROM tb_category c WHERE c.name LIKE 'd%')) AS data
    FROM tb_order o $$, '{"uncascaded_policy": "full_refresh"}');

CREATE FUNCTION in_sync() RETURNS boolean LANGUAGE sql AS $$
    SELECT NOT EXISTS (SELECT 1 FROM tv_order t FULL JOIN tviews.public__tv_order v USING (pk_order)
                       WHERE t.data IS DISTINCT FROM v.data)
       AND NOT EXISTS (SELECT 1 FROM tv_order_counts t
                       FULL JOIN tviews.public__tv_order_counts v USING (pk_order_counts)
                       WHERE t.data IS DISTINCT FROM v.data)
       AND NOT EXISTS (SELECT 1 FROM tv_order_like t
                       FULL JOIN tviews.public__tv_order_like v USING (pk_order_like)
                       WHERE t.data IS DISTINCT FROM v.data) $$;
-- Full refreshes of each TVIEW so far.
CREATE FUNCTION fulls(tview text) RETURNS bigint LANGUAGE sql AS $$
    SELECT full_refreshes FROM tviews.stats WHERE name = tview $$;
CREATE FUNCTION recomputes(tview text) RETURNS bigint LANGUAGE sql AS $$
    SELECT view_recomputes FROM tviews.stats WHERE name = tview $$;

-- Check the step's full refreshes per TVIEW. tv_order_like reads only name: a write
-- to other columns of tb_category does not reach it.
CREATE TABLE before AS SELECT name, full_refreshes, view_recomputes FROM tviews.stats;
CREATE FUNCTION step(what text, order_full int, counts_full int, like_full int) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE got_order bigint; got_counts bigint; got_like bigint;
BEGIN
    SELECT s.full_refreshes - b.full_refreshes INTO got_order
        FROM tviews.stats s JOIN before b USING (name) WHERE s.name = 'tv_order';
    SELECT s.full_refreshes - b.full_refreshes INTO got_counts
        FROM tviews.stats s JOIN before b USING (name) WHERE s.name = 'tv_order_counts';
    SELECT s.full_refreshes - b.full_refreshes INTO got_like
        FROM tviews.stats s JOIN before b USING (name) WHERE s.name = 'tv_order_like';
    PERFORM must(got_order = order_full,
                 format('%s: tv_order refreshed in full %s times, expected %s', what, got_order, order_full));
    PERFORM must(got_counts = counts_full,
                 format('%s: tv_order_counts refreshed in full %s times, expected %s', what, got_counts, counts_full));
    PERFORM must(got_like = like_full,
                 format('%s: tv_order_like refreshed in full %s times, expected %s', what, got_like, like_full));
    PERFORM must(in_sync(), what || ': a TVIEW out of sync');
    TRUNCATE before;
    INSERT INTO before SELECT name, full_refreshes, view_recomputes FROM tviews.stats;
END $$;

-- 1. A new category no read matches: no full refresh, nothing to recompute.
INSERT INTO tb_category (name, kind) VALUES ('brand-new', 'odd');
SELECT step('insert brand-new', 0, 0, 1);

-- 2. A rename of a category lines use: its orders recomputed, no full refresh.
UPDATE tb_category SET name = name || '.' WHERE pk_category = 5;
SELECT must(recomputes('tv_order') > (SELECT view_recomputes FROM before WHERE name = 'tv_order'),
            'rename: the orders of category 5 not recomputed');
-- (cat5's kind IS NULL: tv_order_counts' second read matches it.)
SELECT step('rename category 5', 0, 1, 1);

-- 3. The old row matches name = 'default': refreshed in full.
UPDATE tb_category SET name = 'default-old' WHERE name = 'default';
SELECT step('rename default away', 1, 1, 1);
SELECT must((SELECT bool_and(data->>'default_category' IS NULL) FROM tv_order), 'default still named');
-- 4. The new row matches.
UPDATE tb_category SET name = 'default' WHERE name = 'default-old';
SELECT step('rename default back', 1, 1, 1);
SELECT must((SELECT bool_and(data->>'default_category' = 'default') FROM tv_order), 'default lost');

-- 5. tv_order_counts' ANDed equality and IN list: cat4 is 'even'.
UPDATE tb_category SET kind = 'odd' WHERE name = 'cat4';
SELECT step('cat4 no longer even', 0, 1, 0);
-- cat3 matches none of its filters (kind 'NULL' changes to 'odd': the old row has
-- kind IS NULL and retired_at IS NULL).
UPDATE tb_category SET kind = 'odd' WHERE name = 'cat3';
SELECT step('cat3 kind set', 0, 1, 0);
-- cat6 matches kind = 'even' but not the IN list, nor kind IS NULL.
UPDATE tb_category SET retired_at = now() WHERE name = 'cat6';
SELECT step('cat6 retired', 0, 0, 0);

-- 6. A delete matching nothing; one matching.
DELETE FROM tb_category WHERE name = 'brand-new';
SELECT step('delete brand-new', 0, 0, 1);
INSERT INTO tb_category (name) VALUES ('scratch');
SELECT step('insert scratch (kind IS NULL)', 0, 1, 1);
DELETE FROM tb_category WHERE name = 'scratch';
SELECT step('delete scratch', 0, 1, 1);

-- 7. The registry still names tb_category all_keys: some writes refresh in full.
SELECT must((SELECT cascade_kinds->>'tb_category' FROM tviews.registry WHERE name = 'tv_order')
            = 'all_keys', 'tb_category is not all_keys');

DROP EXTENSION pg_tviews CASCADE;
\echo 'constant lookup: PASS'
