-- Regression test for issues #157 and #158: a base table the TVIEW reads, and
-- triggers watch, but whose writes reach no TVIEW key.
--
--   #157: tb_line read only by a scalar subquery in the select list
--         (ARRAY(SELECT … FROM tb_line l WHERE l.fk_order = o.pk_order)).
--   #158: tb_line read only through a plain view with an aggregate
--         (LEFT JOIN v_invoice_lines v ON v.fk_invoice = i.pk_invoice).
--
-- Both TVIEWs were created without a word about tb_line (#158 named only the
-- view), and every write to tb_line was dropped silently.
--
-- Correct behaviour: both cascade. The link `l.fk_order = o.pk_order` (inside the
-- subquery, or through the view's GROUP BY key) maps a tb_line row to its order
-- (ADR 0157). A table that nothing links to the key (an uncorrelated subquery) is
-- never dropped silently either: it is named at create time, recorded in
-- tviews.registry.uncascaded_tables, and handled by the TVIEW's uncascaded_policy
-- option, declared at create time and stored with the TVIEW:
--   error         ERROR, nothing is created (the default)
--   warn          WARNING, the TVIEW is created
--   full_refresh  NOTICE, and a write to the table refreshes the whole TVIEW
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_uncascaded_tables.sql
--
-- expect-output: writes to public.tb_flag will not refresh public.tv_report
-- expect-output: read in a subquery, with no condition linking it to the TVIEW key
-- expect-output: issue #157 #158 uncascaded tables: PASS
-- reject-output: will not refresh public.tv_order
-- reject-output: will not refresh public.tv_invoice
-- reject-output: Cascade path from 'v_invoice_lines' unresolvable

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- ── #157: scalar subquery in the select list ────────────────────────────────
SET client_min_messages TO NOTICE;
CREATE TABLE tb_order (
    pk_order bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    ref      text);
CREATE TABLE tb_line (
    pk_line  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_order bigint NOT NULL REFERENCES tb_order,
    pos      int NOT NULL,
    sku      text);
INSERT INTO tb_order (ref) VALUES ('o1'), ('o2');
INSERT INTO tb_line (fk_order, pos, sku) VALUES (1, 1, 'a'), (1, 2, 'b'), (2, 1, 'x');

SELECT pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           ARRAY(SELECT l.sku FROM tb_line l WHERE l.fk_order = o.pk_order ORDER BY l.pos) AS skus,
           jsonb_build_object('ref', o.ref) AS data
    FROM tb_order o $$);

-- ── #158: a view with an aggregate ──────────────────────────────────────────
CREATE TABLE tb_invoice (
    pk_invoice bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    ref        text);
CREATE TABLE tb_invoice_line (
    pk_invoice_line bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id              uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_invoice      bigint NOT NULL REFERENCES tb_invoice,
    pos             int NOT NULL,
    sku             text);
INSERT INTO tb_invoice (ref) VALUES ('i1');
INSERT INTO tb_invoice_line (fk_invoice, pos, sku) VALUES (1, 1, 'a'), (1, 2, 'b');
CREATE VIEW v_invoice_lines AS
    SELECT l.fk_invoice, jsonb_agg(jsonb_build_object('sku', l.sku) ORDER BY l.pos) AS lines
    FROM tb_invoice_line l GROUP BY l.fk_invoice;

SELECT pg_tviews_create('tv_invoice', $$
    SELECT i.pk_invoice, i.id, jsonb_build_object('ref', i.ref, 'lines', v.lines) AS data
    FROM tb_invoice i LEFT JOIN v_invoice_lines v ON v.fk_invoice = i.pk_invoice $$);

RESET client_min_messages;
SET client_min_messages TO WARNING;

-- ── TVIEWs whose every base table cascades: no WARNING, empty set ───────────
CREATE TABLE tb_user (
    pk_user bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text);
CREATE TABLE tb_post (
    pk_post bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user bigint REFERENCES tb_user,
    title   text);
INSERT INTO tb_user (name) VALUES ('u1');
INSERT INTO tb_post (fk_user, title) VALUES (1, 'p1');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
-- Embeds another TVIEW's view (entity propagation) and joins a base table.
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.data, 'author_name', bu.name) AS data
    FROM tb_post p
    LEFT JOIN tv_user u ON u.pk_user = p.fk_user
    LEFT JOIN tb_user bu ON bu.pk_user = p.fk_user $$);
SELECT pg_tviews_create_aggregate('tv_user_posts', $$
    SELECT p.fk_user AS pk_user_posts, u.id, jsonb_build_object('posts', count(*)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user
    GROUP BY p.fk_user, u.id
$$, '{"tb_post": "fk_user", "tb_user": "pk_user"}');

-- An uncorrelated subquery: nothing links tb_flag to the key; the TVIEW accepts
-- stale rows (warn).
CREATE TABLE tb_flag (pk_flag bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, active boolean);
CREATE TABLE tb_report (pk_report bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                        id uuid NOT NULL DEFAULT gen_random_uuid());
INSERT INTO tb_report DEFAULT VALUES;
INSERT INTO tb_flag (active) VALUES (true);
SELECT pg_tviews_create_or_replace('tv_report', $$
    SELECT r.pk_report, r.id, jsonb_build_object('flags', (SELECT count(*) FROM tb_flag)) AS data
    FROM tb_report r $$, '{"uncascaded_policy": "warn"}');

-- Helpers.
CREATE FUNCTION _diverges(entity text) RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE d boolean;
BEGIN
    -- Every column of the view, compared with the same column of the TVIEW.
    EXECUTE format('SELECT EXISTS (SELECT 1 FROM tv_%1$s t FULL JOIN tviews.public__tv_%1$s v USING (pk_%1$s) '
                   'WHERE to_jsonb(v.*) IS DISTINCT FROM (SELECT jsonb_object_agg(k, to_jsonb(t.*) -> k) '
                   'FROM jsonb_object_keys(to_jsonb(v.*)) k))', entity) INTO d;
    RETURN d;
END $$;
CREATE FUNCTION _expect_fresh(entity text, label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF _diverges(entity) THEN
        RAISE EXCEPTION 'FAIL [%]: tv_% diverges from v_%', label, entity, entity;
    END IF;
END $$;

-- ── full_refresh: a write to the uncascaded table refreshes the whole TVIEW ─
CREATE TABLE tb_basket (
    pk_basket bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        uuid NOT NULL DEFAULT gen_random_uuid(),
    ref       text);
CREATE TABLE tb_basket_item (
    pk_basket_item bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id             uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_basket      bigint NOT NULL REFERENCES tb_basket,
    sku            text);
INSERT INTO tb_basket (ref) SELECT 'b' || g FROM generate_series(1, 50) g;
INSERT INTO tb_basket_item (fk_basket, sku) SELECT 1 + g % 50, 's' || g FROM generate_series(1, 200) g;

-- Each basket shows the share of all items it holds: the uncorrelated count links
-- tb_basket_item to no key. The declared option is stored with the TVIEW, and
-- every later write uses it.
SELECT pg_tviews_create('tv_basket', $$
    SELECT b.pk_basket, b.id,
           jsonb_build_object('ref', b.ref,
               'items', (SELECT count(*) FROM tb_basket_item i WHERE i.fk_basket = b.pk_basket),
               'of', (SELECT count(*) FROM tb_basket_item)) AS data
    FROM tb_basket b $$, '{"uncascaded_policy": "full_refresh"}');
DO $$ BEGIN
    IF (SELECT options ->> 'uncascaded_policy' FROM tviews.registry WHERE entity = 'basket')
       IS DISTINCT FROM 'full_refresh' THEN
        RAISE EXCEPTION 'FAIL [full_refresh]: the declared option is not stored';
    END IF;
END $$;
UPDATE tb_basket_item SET fk_basket = 2 WHERE fk_basket = 1;
SELECT _expect_fresh('basket', 'full_refresh: UPDATE moves items');
INSERT INTO tb_basket_item (fk_basket, sku) VALUES (3, 'new');
SELECT _expect_fresh('basket', 'full_refresh: INSERT');
BEGIN;
DELETE FROM tb_basket_item WHERE fk_basket = 4;
UPDATE tb_basket_item SET sku = 'z' WHERE fk_basket = 5;
COMMIT;
SELECT _expect_fresh('basket', 'full_refresh: explicit transaction');
DELETE FROM tb_basket_item WHERE fk_basket = 6;
SELECT _expect_fresh('basket', 'full_refresh: a later autocommit DELETE');

-- ── error: nothing is created ───────────────────────────────────────────────
CREATE TABLE tb_shelf (
    pk_shelf bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid());
CREATE TABLE tb_book (
    pk_book  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    fk_shelf bigint REFERENCES tb_shelf,
    title    text);
DO $$
BEGIN
    PERFORM pg_tviews_create('tv_shelf', $q$
        SELECT s.pk_shelf, s.id,
               jsonb_build_object('books', ARRAY(SELECT b.title FROM tb_book b)) AS data
        FROM tb_shelf s $q$);
    RAISE EXCEPTION 'FAIL [error]: the TVIEW was created';
EXCEPTION WHEN OTHERS THEN
    IF SQLERRM LIKE 'FAIL%' THEN RAISE; END IF;
    IF SQLERRM NOT LIKE '%public.tb_book%' THEN
        RAISE EXCEPTION 'FAIL [error]: the ERROR does not name tb_book: %', SQLERRM;
    END IF;
END $$;
DO $$ BEGIN
    IF to_regclass('public.tv_shelf') IS NOT NULL OR to_regclass('tviews.public__tv_shelf') IS NOT NULL
       OR EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'shelf')
       OR EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid IN ('tb_shelf'::regclass, 'tb_book'::regclass)
                  AND NOT tgisinternal)
    THEN
        RAISE EXCEPTION 'FAIL [error]: a rejected create left objects behind';
    END IF;
END $$;

-- ── #157 and #158: writes to tb_line / tb_invoice_line refresh ───────────────
UPDATE tb_line SET sku = 'b2' WHERE pos = 2;
SELECT _expect_fresh('order', '#157 UPDATE');
INSERT INTO tb_line (fk_order, pos, sku) VALUES (1, 3, 'c');
SELECT _expect_fresh('order', '#157 INSERT');
UPDATE tb_line SET fk_order = 2 WHERE pos = 1 AND fk_order = 1;   -- moves to order 2
SELECT _expect_fresh('order', '#157 fk move');
DELETE FROM tb_line WHERE sku = 'x';
SELECT _expect_fresh('order', '#157 DELETE');
UPDATE tb_invoice_line SET sku = 'b3' WHERE pos = 2;
SELECT _expect_fresh('invoice', '#158 UPDATE');
INSERT INTO tb_invoice_line (fk_invoice, pos, sku) VALUES (1, 3, 'c');
DELETE FROM tb_invoice_line WHERE pos = 1;
SELECT _expect_fresh('invoice', '#158 INSERT, DELETE');

-- ── warn: an unlinked table's writes leave the TVIEW stale, as stated ────────
UPDATE tb_flag SET active = false;
INSERT INTO tb_flag (active) VALUES (true);
DO $$ BEGIN
    IF NOT _diverges('report') THEN
        RAISE EXCEPTION 'FAIL [warn]: expected tv_report to stay stale under the warn policy';
    END IF;
END $$;

-- ── registry: the sets and the stored policies ──────────────────────────────
DO $$
DECLARE r record;
BEGIN
    FOR r IN SELECT entity, uncascaded_tables, uncascaded_policy FROM tviews.registry LOOP
        IF r.uncascaded_tables IS DISTINCT FROM (CASE r.entity
                WHEN 'report'  THEN ARRAY['tb_flag'::regclass]
                WHEN 'basket'  THEN ARRAY['tb_basket_item'::regclass]
                ELSE '{}'::regclass[] END) THEN
            RAISE EXCEPTION 'FAIL [registry]: % uncascaded_tables = %', r.entity, r.uncascaded_tables;
        END IF;
        IF r.uncascaded_policy IS DISTINCT FROM
           (CASE r.entity WHEN 'basket' THEN 'full_refresh' WHEN 'report' THEN 'warn'
                          ELSE 'error' END) THEN
            RAISE EXCEPTION 'FAIL [registry]: % uncascaded_policy = %', r.entity, r.uncascaded_policy;
        END IF;
    END LOOP;
END $$;

-- ── re-registration recomputes the set and keeps the stored policy ──────────
SELECT pg_tviews_reregister('basket');
SELECT pg_tviews_reregister('report');
DO $$ BEGIN
    IF (SELECT uncascaded_policy FROM tviews.registry WHERE entity = 'basket') <> 'full_refresh'
       OR (SELECT uncascaded_tables FROM tviews.registry WHERE entity = 'report')
          IS DISTINCT FROM ARRAY['tb_flag'::regclass]
    THEN
        RAISE EXCEPTION 'FAIL [reregister]: the set or the stored policy changed';
    END IF;
END $$;
UPDATE tb_basket_item SET sku = 'after-reregister' WHERE fk_basket = 7;
SELECT _expect_fresh('basket', 'full_refresh after reregister');

\echo 'issue #157 #158 uncascaded tables: PASS'
