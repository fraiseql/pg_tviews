-- The order rule (dialect.md Axis D, D13): wherever the order of rows reaches the
-- output (an order-sensitive aggregate, LIMIT, DISTINCT ON, an order-sensitive
-- window function), it must be total, or the stored document is one of several the
-- view may return, and an UPDATE that moves a row in the heap changes which. A
-- site is accepted when its sort keys, closed under the level's equalities and
-- the tables' unique keys, determine every row it orders or every value it reads
-- (rule 1: rows that tie are then identical where it looks), when at most one row
-- reaches it (rule 2), when an aggregate is ordered by (or DISTINCT on)
-- what it aggregates and the values render as they compare (rule 3), or when a
-- UNION ALL of single-row branches is ordered by a constant rank (rule 4).
-- Otherwise it is refused with 0A000, naming the site and a key to append.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_order_rule.sql
--
-- expect-output: order rule: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, label text NOT NULL);
CREATE TABLE tb_p (pk_p int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text,
                   tags jsonb NOT NULL DEFAULT '[]');
CREATE TABLE tb_c (pk_c int PRIMARY KEY, fk_p int NOT NULL REFERENCES tb_p,
                   fk_tag int REFERENCES tb_tag, label text, note text, amount numeric,
                   weight float8, pos int NOT NULL DEFAULT 0, code text UNIQUE,
                   deleted boolean NOT NULL DEFAULT false);
CREATE UNIQUE INDEX tb_c_live_pos ON tb_c (fk_p, pos) WHERE NOT deleted;
CREATE TABLE tb_name (pk_name int PRIMARY KEY, fk_p int NOT NULL REFERENCES tb_p,
                      locale text NOT NULL, label text, UNIQUE (fk_p, locale));
CREATE TABLE tb_link (pk_link int PRIMARY KEY, fk_p int REFERENCES tb_p, fk_tag int REFERENCES tb_tag,
                      note text, deleted_at timestamptz);
CREATE UNIQUE INDEX tb_link_live ON tb_link (fk_p, fk_tag) WHERE deleted_at IS NULL;
INSERT INTO tb_tag VALUES (1, 't1'), (2, 't2');
-- Grouped views: by the key and what it fixes, and by a column nothing fixes.
CREATE VIEW v_c_counted AS
  SELECT c.pk_c, c.fk_p, t.label, count(*) AS n FROM tb_c c LEFT JOIN tb_tag t ON t.pk_tag = c.fk_tag
  GROUP BY c.pk_c, c.fk_p, c.note, t.label;
CREATE VIEW v_c_by_label AS
  SELECT c.fk_p, c.label, count(*) AS n FROM tb_c c GROUP BY c.fk_p, c.label, c.note;
INSERT INTO tb_p (pk_p, name, tags) VALUES (1, 'p', '["x", "y"]'), (2, 'q', '[]');
INSERT INTO tb_c VALUES (1, 1, 1, 'a', NULL, 1.0, 0.5, 1, 'c1', false),
                        (2, 1, 2, 'b', NULL, 1.00, 0.25, 2, 'c2', false),
                        (3, 2, 1, 'a', NULL, 2, 1, 1, NULL, true);
INSERT INTO tb_name VALUES (1, 1, 'fr', 'nom'), (2, 1, 'en', 'name');

-- Raise unless creating tv_p with `data` (its data expression) is refused with
-- 0A000 and a message matching `like`.
CREATE FUNCTION refused(data text, like_ text, what text) RETURNS void LANGUAGE plpgsql AS $f$
DECLARE state text; msg text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_create_or_replace('tv_p',
            format('SELECT p.pk_p, p.id, %s AS data FROM tb_p p', data));
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, msg = MESSAGE_TEXT;
        IF state <> '0A000' OR msg NOT LIKE like_ THEN
            RAISE EXCEPTION 'FAIL: % failed with % (want 0A000, %): %', what, state, like_, msg;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: % was accepted', what;
END $f$;

-- Raise unless creating tv_p with `data` is accepted and stays fresh after an
-- UPDATE of a column the definition does not read.
CREATE FUNCTION accepted(data text, what text) RETURNS void LANGUAGE plpgsql AS $f$
DECLARE diff text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_create_or_replace('tv_p',
            format('SELECT p.pk_p, p.id, %s AS data FROM tb_p p', data));
    EXCEPTION WHEN OTHERS THEN
        RAISE EXCEPTION 'FAIL: % was refused: %', what, SQLERRM;
    END;
    UPDATE tb_c SET note = coalesce(note, '') || 'x';
    diff := fresh_diff('tv_p', 'pk_p');
    IF diff IS NOT NULL THEN
        RAISE EXCEPTION 'FAIL: % went stale: %', what, diff;
    END IF;
END $f$;

-- Rule 1: the sort keys determine every row.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(c.label ORDER BY c.pk_c)
                                            FROM tb_c c WHERE c.fk_p = p.pk_p))$$, 'ordered by the key');
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(t.label ORDER BY c.pk_c)
                                            FROM tb_c c JOIN tb_tag t ON t.pk_tag = c.fk_tag
                                            WHERE c.fk_p = p.pk_p))$$, 'a to-one join through its key');
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(c.label ORDER BY c.pos)
                                            FROM tb_c c WHERE c.fk_p = p.pk_p AND NOT c.deleted))$$,
                'a partial unique index the filter implies');
-- The nullable side of a LEFT JOIN: its ON condition holds for every row it joins.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(n.label ORDER BY c.pk_c) FROM tb_c c
                                            LEFT JOIN tb_c n ON n.fk_p = c.fk_p AND n.pos = c.pos + 1
                                             AND NOT n.deleted
                                            WHERE c.fk_p = p.pk_p))$$,
                'a partial unique index a LEFT JOIN condition implies');
SELECT accepted($$jsonb_build_object('l', (SELECT c.label FROM tb_c c WHERE c.fk_p = p.pk_p
                                            ORDER BY c.pk_c LIMIT 1))$$, 'LIMIT 1 ordered by the key');
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(d.label ORDER BY d.fk_p)
                                            FROM (SELECT DISTINCT ON (c.fk_p) c.fk_p, c.label FROM tb_c c
                                                  ORDER BY c.fk_p, c.pk_c) d WHERE d.fk_p = p.pk_p))$$,
                'DISTINCT ON ordered by the key, its output keyed by its DISTINCT ON column');
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(r.label ORDER BY r.pk_c)
                                            FROM (SELECT c.label, row_number() OVER (PARTITION BY c.fk_p ORDER BY c.pk_c) AS n,
                                                         c.pk_c
                                                  FROM tb_c c WHERE c.fk_p = p.pk_p) r))$$,
                'row_number ordered by the key');
SELECT accepted($$jsonb_build_object('t', (SELECT jsonb_agg(e.v ORDER BY e.n)
                                            FROM jsonb_array_elements_text(p.tags) WITH ORDINALITY e(v, n)))$$,
                'a function in FROM WITH ORDINALITY');
SELECT accepted($$jsonb_build_object('l', ARRAY(SELECT c.label FROM tb_c c WHERE c.fk_p = p.pk_p
                                                  ORDER BY c.pos, c.pk_c))$$, 'ARRAY(SELECT …) ordered by the key');
-- Rows that tie but read the same: an unnest joined on its element (every column of
-- it known), and ties that differ only in what the aggregate does not read.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(c.label ORDER BY c.pk_c)
                                            FROM unnest(ARRAY[1, 2, 1]) u(x)
                                            JOIN tb_c c ON c.pk_c = u.x WHERE c.fk_p = p.pk_p))$$,
                'an unnest joined on its element');
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(c.label ORDER BY c.pk_c)
                                            FROM unnest(ARRAY['1', '01']) u(x)
                                            JOIN tb_c c ON c.pk_c = u.x::int WHERE c.fk_p = p.pk_p))$$,
                'ties that differ only in a column not aggregated');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(u.x ORDER BY c.pk_c)
                                           FROM unnest(ARRAY['1', '01']) u(x)
                                           JOIN tb_c c ON c.pk_c = u.x::int WHERE c.fk_p = p.pk_p))$$,
               '%jsonb_agg()%u (a subquery, CTE or function without a key)%',
               'ties that differ in what is aggregated');
-- ... and at the other sites: the row kept, or the element listed, is the same.
SELECT accepted($$jsonb_build_object('l', ARRAY(SELECT c.label FROM tb_c c WHERE c.fk_p = p.pk_p
                                                  ORDER BY c.label))$$, 'ARRAY(SELECT …) of what it orders by');
SELECT accepted($$jsonb_build_object('l', (SELECT c.label FROM tb_c c WHERE c.fk_p = p.pk_p
                                            ORDER BY c.label LIMIT 1))$$, 'LIMIT 1 of what it orders by');
SELECT accepted($$jsonb_build_object('l', (SELECT DISTINCT ON (c.fk_p) c.label FROM tb_c c
                                            WHERE c.fk_p = p.pk_p ORDER BY c.fk_p, c.label))$$,
                'DISTINCT ON keeping what it orders by');
SELECT refused($$jsonb_build_object('l', ARRAY(SELECT c.amount FROM tb_c c WHERE c.fk_p = p.pk_p
                                                 ORDER BY c.amount))$$, '%ARRAY(SELECT%pk_c%',
               'ARRAY(SELECT …) of a numeric it orders by: 1.0 and 1.00 tie');
-- The hint names the keys to append and no more: tb_tag follows from tb_c's key.
SELECT refused($$jsonb_build_object('l', (SELECT t.label FROM tb_c c JOIN tb_tag t ON t.pk_tag = c.fk_tag
                                           WHERE c.fk_p = p.pk_p ORDER BY c.pos LIMIT 1))$$,
               '%LIMIT%every row: public.tb_c c (append c.pk_c)', 'the minimal hint');
-- A key equated with an uncorrelated scalar subquery: one value for every row.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(n.label ORDER BY c.pk_c) FROM tb_c c
                                            LEFT JOIN tb_name n ON n.fk_p = c.fk_p
                                             AND n.locale = (SELECT 'en'::text)
                                            WHERE c.fk_p = p.pk_p))$$, 'a key equated with a scalar subquery');
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(n.label ORDER BY c.pk_c) FROM tb_c c
                                            LEFT JOIN tb_name n ON n.fk_p = c.fk_p
                                             AND n.locale = (SELECT c.label)
                                            WHERE c.fk_p = p.pk_p))$$,
                'a key equated with a scalar subquery reading a known column');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(n.label ORDER BY c.pk_c) FROM tb_c c
                                           LEFT JOIN tb_name n ON n.fk_p = c.fk_p
                                            AND n.locale = (SELECT x.label FROM tb_c x WHERE x.fk_p = c.fk_p LIMIT 1)
                                           WHERE c.fk_p = p.pk_p))$$, '%LIMIT%',
               'a key equated with a scalar subquery picking a row in no order');
-- A derived table of several rows per key (a set-returning output): its key still
-- fixes its other outputs, so DISTINCT ON over groups of them picks among equals.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(d.label ORDER BY d.pk_c) FROM (
                                              SELECT DISTINCT ON (s.pk_c) s.pk_c, s.label FROM (
                                                SELECT c.pk_c, c.fk_p, c.label, unnest(ARRAY[1, 2]) AS k FROM tb_c c) s
                                              WHERE s.fk_p = p.pk_p GROUP BY s.pk_c, s.label ORDER BY s.pk_c) d))$$,
                'DISTINCT ON over groups its key fixes, through a derived table');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(d.label ORDER BY d.pk_c) FROM (
                                             SELECT DISTINCT ON (s.pk_c) s.pk_c, s.label FROM (
                                               SELECT c.pk_c, c.fk_p, n.label, unnest(ARRAY[1, 2]) AS k
                                               FROM tb_c c JOIN tb_name n ON n.fk_p = c.fk_p) s
                                             WHERE s.fk_p = p.pk_p GROUP BY s.pk_c, s.label ORDER BY s.pk_c) d))$$,
               '%DISTINCT ON%its groups (1 of its 2 grouping or DISTINCT keys%',
               'DISTINCT ON over groups its key leaves open, through a derived table');
-- A grouped view named by its key: the key fixes every grouping column.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(v.n ORDER BY v.pk_c) FROM v_c_counted v
                                            WHERE v.fk_p = p.pk_p))$$, 'a grouped view ordered by its key');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(v.n ORDER BY v.label) FROM v_c_by_label v
                                           WHERE v.fk_p = p.pk_p))$$, '%jsonb_agg%v_c_by_label%',
               'a grouped view ordered by a column leaving a grouping column open');
-- A CTE picking one row of a ranked UNION ALL: at most one row, joined to every row.
SELECT accepted($$jsonb_build_object('l', (WITH pick AS (SELECT r.label FROM (
                                              SELECT 1 AS rank, n.label FROM tb_name n WHERE n.fk_p = p.pk_p AND n.locale = 'en'
                                              UNION ALL
                                              SELECT 2, n.label FROM tb_name n WHERE n.fk_p = p.pk_p AND n.locale = 'fr'
                                              ORDER BY rank LIMIT 1) r)
                                            SELECT jsonb_agg(pick.label || c.label ORDER BY c.pk_c)
                                            FROM tb_c c CROSS JOIN pick WHERE c.fk_p = p.pk_p))$$,
                'a CTE of one ranked row joined to every row');
-- A unique index over nullable columns keys the rows its columns are not NULL in.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(l.note ORDER BY t.pk_tag) FROM tb_link l
                                            JOIN tb_tag t ON t.pk_tag = l.fk_tag
                                            WHERE l.fk_p = p.pk_p AND l.deleted_at IS NULL))$$,
                'a nullable key whose columns equalities fix');
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(l.note ORDER BY l.fk_tag) FROM tb_link l
                                            WHERE l.fk_p = p.pk_p AND l.deleted_at IS NULL
                                              AND l.fk_tag IS NOT NULL))$$,
                'a nullable key ordered by a column filtered not NULL');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(l.note ORDER BY l.fk_tag) FROM tb_link l
                                           WHERE l.fk_p = p.pk_p AND l.deleted_at IS NULL))$$,
               '%jsonb_agg%tb_link l (append l.pk_link)%', 'a nullable key ordered by a column that may be NULL');
-- Rule 4 with each branch's rank written in the subquery or CTE it reads.
SELECT accepted($$jsonb_build_object('l', (SELECT u.label FROM (
                                              SELECT e.rank, e.label FROM (SELECT 1 AS rank, n.label FROM tb_name n
                                                WHERE n.fk_p = p.pk_p AND n.locale = 'en') e
                                              UNION ALL
                                              SELECT f.rank, f.label FROM (SELECT 2 AS rank, n.label FROM tb_name n
                                                WHERE n.fk_p = p.pk_p AND n.locale = 'fr') f) u
                                            ORDER BY u.rank LIMIT 1))$$, 'ranks written in the subqueries the branches read');
SELECT refused($$jsonb_build_object('l', (SELECT u.label FROM (
                                             SELECT e.rank, e.label FROM (SELECT 1 AS rank, n.label FROM tb_name n
                                               WHERE n.fk_p = p.pk_p AND n.locale = 'en') e
                                             UNION ALL
                                             SELECT f.rank, f.label FROM (SELECT 1 AS rank, n.label FROM tb_name n
                                               WHERE n.fk_p = p.pk_p AND n.locale = 'fr') f) u
                                           ORDER BY u.rank LIMIT 1))$$, '%LIMIT%u (a subquery, CTE or function without a key)%',
               'the same rank written in both subqueries');
-- Rule 2: at most one row.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(c.label) FROM tb_c c WHERE c.pk_c = p.pk_p))$$,
                'one row by its key');
SELECT accepted($$jsonb_build_object('l', (SELECT n.label FROM tb_name n WHERE n.fk_p = p.pk_p
                                            AND n.locale = 'fr' LIMIT 1))$$, 'one row by a unique pair');
-- Rule 3: ordered by what it aggregates, a type that renders as it compares.
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(c.label ORDER BY c.label)
                                            FROM tb_c c WHERE c.fk_p = p.pk_p))$$, 'ordered by its text');
SELECT accepted($$jsonb_build_object('l', (SELECT string_agg(DISTINCT c.label, ',')
                                            FROM tb_c c WHERE c.fk_p = p.pk_p))$$, 'DISTINCT text');
-- Rule 4: single-row branches ranked by a constant.
SELECT accepted($$jsonb_build_object('n', (SELECT u.label FROM (
                       SELECT 1 AS rank, n.label FROM tb_name n WHERE n.fk_p = p.pk_p AND n.locale = 'de'
                       UNION ALL
                       SELECT 2, n.label FROM tb_name n WHERE n.fk_p = p.pk_p AND n.locale = 'fr') u
                   ORDER BY u.rank LIMIT 1))$$, 'a locale fallback ranked by a constant');
-- Not sites: OFFSET 0, an order-insensitive aggregate, a top-level ORDER BY.
SELECT accepted($$jsonb_build_object('n', (SELECT count(*) FROM (SELECT c.pk_c FROM tb_c c
                                            WHERE c.fk_p = p.pk_p OFFSET 0) o),
                                     's', (SELECT sum(c.pos) FROM tb_c c WHERE c.fk_p = p.pk_p))$$,
                'OFFSET 0 and sum(int)');
-- A view the definition reads, looked through: keyed by its own key.
CREATE VIEW v_c AS SELECT c.pk_c, c.fk_p, c.label FROM tb_c c WHERE NOT c.deleted;
SELECT accepted($$jsonb_build_object('l', (SELECT jsonb_agg(v.label ORDER BY v.pk_c)
                                            FROM v_c v WHERE v.fk_p = p.pk_p))$$, 'a view looked through');

-- Refused.
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(c.label) FROM tb_c c WHERE c.fk_p = p.pk_p))$$,
               '%jsonb_agg%tb_c%pk_c%', 'an unordered jsonb_agg');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(c.note ORDER BY c.label) FROM tb_c c
                                           WHERE c.fk_p = p.pk_p))$$, '%jsonb_agg%pk_c%',
               'ordered by a column that ties');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(c.amount ORDER BY c.amount) FROM tb_c c
                                           WHERE c.fk_p = p.pk_p))$$, '%jsonb_agg%numeric%',
               'ordered by a numeric: 1.0 and 1.00 tie');
SELECT refused($$jsonb_build_object('l', (SELECT string_agg(c.label, ',') FROM tb_c c
                                           WHERE c.fk_p = p.pk_p))$$, '%string_agg%', 'an unordered string_agg');
SELECT refused($$jsonb_build_object('w', (SELECT sum(c.weight) FROM tb_c c WHERE c.fk_p = p.pk_p))$$,
               '%sum%float%', 'sum of a float8, unordered');
SELECT refused($$jsonb_build_object('l', ARRAY(SELECT c.label FROM tb_c c WHERE c.fk_p = p.pk_p
                                                 ORDER BY c.pos))$$, '%ARRAY(SELECT%pk_c%',
               'ARRAY(SELECT …) ordered by a tie');
SELECT refused($$jsonb_build_object('l', (SELECT c.label FROM tb_c c WHERE c.fk_p = p.pk_p LIMIT 1))$$,
               '%LIMIT%pk_c%', 'LIMIT 1 without ORDER BY');
SELECT refused($$jsonb_build_object('l', (SELECT c.note FROM tb_c c WHERE c.fk_p = p.pk_p
                                           ORDER BY c.label LIMIT 1))$$, '%LIMIT%', 'LIMIT 1 ordered by a tie');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(d.note ORDER BY d.fk_p)
                                           FROM (SELECT DISTINCT ON (c.fk_p) c.fk_p, c.note FROM tb_c c
                                                 ORDER BY c.fk_p, c.label) d WHERE d.fk_p = p.pk_p))$$,
               '%DISTINCT ON%', 'DISTINCT ON ordered by a tie');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(r.n ORDER BY r.pk_c)
                                           FROM (SELECT c.pk_c, row_number() OVER (PARTITION BY c.fk_p ORDER BY c.label) AS n
                                                 FROM tb_c c WHERE c.fk_p = p.pk_p) r))$$,
               '%row_number%', 'row_number ordered by a tie');
SELECT refused($$jsonb_build_object('n', (SELECT u.label FROM (
                      SELECT n.label FROM tb_name n WHERE n.fk_p = p.pk_p AND n.locale = 'de'
                      UNION ALL
                      SELECT n.label FROM tb_name n WHERE n.fk_p = p.pk_p AND n.locale = 'fr') u
                  LIMIT 1))$$, '%LIMIT%', 'a UNION ALL … LIMIT 1 without a rank');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(c.label ORDER BY c.pos)
                                           FROM tb_c c WHERE c.fk_p = p.pk_p))$$, '%jsonb_agg%',
               'a partial unique index the filter does not imply');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(n.label ORDER BY c.pk_c) FROM tb_c c
                                           LEFT JOIN tb_c n ON n.fk_p = c.fk_p AND n.pos = c.pos + 1
                                           WHERE c.fk_p = p.pk_p))$$, '%jsonb_agg%tb_c n%',
               'a partial unique index no LEFT JOIN condition implies');
SELECT refused($$jsonb_build_object('l', (SELECT c.label FROM tb_c c WHERE c.fk_p = p.pk_p
                                           GROUP BY c.label, c.note ORDER BY c.label LIMIT 1))$$,
               '%LIMIT%its groups (1 of its 2 grouping or DISTINCT keys%', 'LIMIT over groups ordered by one key');
SELECT refused($$jsonb_build_object('l', (SELECT jsonb_agg(c.label ORDER BY c.code)
                                           FROM tb_c c WHERE c.fk_p = p.pk_p))$$, '%jsonb_agg%',
               'a unique column without NOT NULL');
CREATE VIEW v_unordered AS SELECT p.pk_p, (SELECT jsonb_agg(c.label) FROM tb_c c WHERE c.fk_p = p.pk_p) AS labels
    FROM tb_p p;
SELECT refused($$jsonb_build_object('l', (SELECT u.labels FROM v_unordered u WHERE u.pk_p = p.pk_p))$$,
               '%jsonb_agg%v_unordered%', 'in a view the definition reads');

SELECT 'order rule: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
