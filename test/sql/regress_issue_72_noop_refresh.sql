-- Regression test for issue #72:
--   "Refresh rewrites unchanged rows: no IS DISTINCT FROM guard, updated_at
--    bumped unconditionally"
--
-- Every refresh path wrote the tv_* row even when its content would not change:
-- a new tuple version, index entries and a dead tuple per key, and updated_at
-- only recorded that a refresh ran.
--
-- Correct behaviour: a refresh whose result equals the stored row writes
-- nothing (n_tup_upd unchanged, updated_at unchanged), on every path: bulk
-- recompute, cascade, direct patch, smart patch, and DISTINCT ON upsert. A real
-- change (including NULL <-> value) still writes and moves updated_at.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_72_noop_refresh.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_post, tv_article, tv_user, tv_item, tv_contract CASCADE;
DROP VIEW  IF EXISTS v_post, v_article, v_user, v_item, v_contract CASCADE;
DROP TABLE IF EXISTS tb_post, tb_article, tb_user, tb_item, tb_contract CASCADE;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER REFERENCES tb_user(pk_user),
    title   TEXT
);
-- Scalar dependency (base-table join, no v_* embed): single-key refreshes take
-- the smart-patch upsert.
CREATE TABLE tb_article (
    pk_article INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user    INTEGER REFERENCES tb_user(pk_user),
    title      TEXT
);
-- No dependencies: eligible for the #56 direct-patch fast path (text/jsonb columns).
CREATE TABLE tb_item (
    pk_item INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    meta    JSONB,
    label   TEXT
);
CREATE TABLE tb_contract (
    pk_contract INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id          UUID DEFAULT gen_random_uuid() NOT NULL,
    id_contract INTEGER NOT NULL,
    version_no  INTEGER NOT NULL,
    status      TEXT
);

INSERT INTO tb_user (name) SELECT 'user ' || g FROM generate_series(1, 10) g;
INSERT INTO tb_post (fk_user, title) SELECT 1 + g % 10, 'post ' || g FROM generate_series(1, 200) g;
INSERT INTO tb_article (fk_user, title) SELECT 1 + g % 10, 'article ' || g FROM generate_series(1, 20) g;
INSERT INTO tb_item (meta, label) SELECT '{"k": 1.0}', 'item ' || g FROM generate_series(1, 20) g;
INSERT INTO tb_contract (id_contract, version_no, status) VALUES (100, 1, 'draft'), (100, 2, 'active');

SELECT pg_tviews_create('tv_user', $TV$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$TV$);
SELECT pg_tviews_create('tv_post', $TV$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object('title', tb_post.title, 'author', v_user.data) AS data
    FROM tb_post LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
$TV$);
SELECT pg_tviews_create('tv_article', $TV$
    SELECT a.pk_article, a.id, a.fk_user,
           jsonb_build_object('title', a.title, 'author_name', u.name) AS data
    FROM tb_article a JOIN tb_user u ON u.pk_user = a.fk_user
$TV$);
SELECT pg_tviews_create('tv_item', $TV$
    SELECT pk_item, id, jsonb_build_object('meta', meta, 'label', label) AS data FROM tb_item
$TV$);

-- Preconditions: the classifications that route refreshes to the intended paths.
DO $$ BEGIN
    IF (SELECT dependency_types FROM pg_tview_meta WHERE entity = 'article') <> ARRAY['scalar'] THEN
        RAISE EXCEPTION 'setup: tv_article is not a scalar-dependency tview';
    END IF;
    IF (SELECT dependency_types FROM pg_tview_meta WHERE entity = 'item') <> '{}' THEN
        RAISE EXCEPTION 'setup: tv_item has dependencies';
    END IF;
END $$;
SELECT pg_tviews_create('tv_contract', $TV$
    SELECT DISTINCT ON (c.id_contract)
           c.id_contract AS pk_contract, c.id,
           jsonb_build_object('status', c.status, 'version', c.version_no) AS data
    FROM tb_contract c
    ORDER BY c.id_contract, c.version_no DESC
$TV$);

-- Bookkeeping: tuple updates per tview since the last mark, plus updated_at.
CREATE TABLE _mark (relname text PRIMARY KEY, upd bigint, max_updated timestamptz);
CREATE FUNCTION _tv_upd(rel text) RETURNS bigint LANGUAGE sql AS $$
    SELECT n_tup_upd FROM pg_stat_user_tables WHERE relname = rel
$$;
CREATE FUNCTION _max_updated(rel text) RETURNS timestamptz LANGUAGE plpgsql AS $$
DECLARE t timestamptz;
BEGIN
    EXECUTE format('SELECT max(updated_at) FROM %I', rel) INTO t;
    RETURN t;
END $$;
CREATE FUNCTION _set_mark() RETURNS void LANGUAGE sql AS $$
    DELETE FROM _mark;
    INSERT INTO _mark
    SELECT r, _tv_upd(r), _max_updated(r)
    FROM unnest(ARRAY['tv_user', 'tv_post', 'tv_article', 'tv_item', 'tv_contract']) r;
$$;
-- Assert tview `rel` got exactly `expected` tuple updates since the mark, and
-- that updated_at moved iff it was written.
CREATE FUNCTION _expect(rel text, expected bigint, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE got bigint; moved boolean;
BEGIN
    PERFORM pg_stat_clear_snapshot();
    SELECT _tv_upd(rel) - m.upd, _max_updated(rel) IS DISTINCT FROM m.max_updated
      INTO got, moved FROM _mark m WHERE m.relname = rel;
    IF got <> expected THEN
        RAISE EXCEPTION 'FAIL #72 [%]: % got % tuple updates, expected %', label, rel, got, expected;
    END IF;
    IF moved <> (expected > 0) THEN
        RAISE EXCEPTION 'FAIL #72 [%]: % updated_at moved=% with % writes', label, rel, moved, got;
    END IF;
END $$;
CREATE FUNCTION _recomputes() RETURNS bigint LANGUAGE sql AS
    $$ SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint $$;
-- Session-cumulative count of refresh writes skipped because nothing changed.
CREATE FUNCTION _skipped() RETURNS bigint LANGUAGE sql AS
    $$ SELECT (pg_tviews_queue_stats()->>'refresh_noop_skipped')::bigint $$;
CREATE TABLE _skip0 (v bigint);
CREATE FUNCTION _expect_skipped(expected bigint, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE got bigint := _skipped() - (SELECT v FROM _skip0);
BEGIN
    IF got IS DISTINCT FROM expected THEN
        RAISE EXCEPTION 'FAIL #72 [%]: refresh_noop_skipped grew by %, expected %', label, got, expected;
    END IF;
END $$;

-- Stats are published when the backend goes idle; every check below is
--   flush; mark; <mutation>; flush; expect
-- with each step its own statement.

-- ── 1. bulk recompute: no-op UPDATE over 200 rows ───────────────────────────
SELECT pg_stat_force_next_flush();
SELECT _set_mark();
INSERT INTO _skip0 SELECT _skipped();
UPDATE tb_post SET title = title;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_post', 0, 'bulk no-op');
SELECT _expect_skipped(200, 'bulk no-op');

-- ── 2. same no-op, repeated ─────────────────────────────────────────────────
UPDATE tb_post SET title = title;
UPDATE tb_post SET title = title;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_post', 0, 'bulk no-op x3');

-- ── 3. cascade: embedded column set to its current value ────────────────────
SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_user SET name = name WHERE pk_user = 1;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_user', 0, 'cascade no-op (own)');
SELECT _expect('tv_post', 0, 'cascade no-op (parent)');

-- ── 4. direct patch: jsonb datum changes (1.0 -> 1.00), jsonb value does not ─
-- The captured column differs byte-wise, so the #56 fast path builds a patch;
-- the patched document equals the stored one.
SET pg_tviews.direct_patch_enabled = on;
CREATE TABLE _rc0 AS SELECT _recomputes() AS v;
SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE _skip0 SET v = _skipped();
UPDATE tb_item SET meta = '{"k": 1.00}' WHERE pk_item <= 5;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_item', 0, 'direct-patch no-op');
SELECT _expect_skipped(5, 'direct-patch no-op');
DO $$ BEGIN
    IF _recomputes() <> (SELECT v FROM _rc0) THEN
        RAISE EXCEPTION 'FAIL #72 [direct-patch no-op]: % view recompute(s); an unchanged '
                        'materialised row must not fall back to recompute',
                        _recomputes() - (SELECT v FROM _rc0);
    END IF;
END $$;

-- ── 5. single-key paths (direct patch off) ──────────────────────────────────
-- One key per flush goes through refresh_pk: full replacement for a tview
-- without dependencies, smart-patch upsert for a scalar-dependency tview.
SET pg_tviews.direct_patch_enabled = off;
SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_item SET label = label WHERE pk_item = 6;
UPDATE tb_article SET title = title WHERE pk_article = 1;
UPDATE tb_user SET name = name WHERE pk_user = 2;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_item', 0, 'full-replacement no-op');
SELECT _expect('tv_article', 0, 'smart-patch no-op');
SELECT _expect('tv_post', 0, 'cascade no-op, direct patch off');
SET pg_tviews.direct_patch_enabled = on;

-- ── 6. DISTINCT ON upsert ───────────────────────────────────────────────────
SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_contract SET status = status WHERE id_contract = 100;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_contract', 0, 'distinct-on no-op');

-- ── 7. positive controls: real changes still write and move updated_at ──────
SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_post SET title = 'changed' WHERE pk_post = 1;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_post', 1, 'real change');

SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_post SET title = NULL WHERE pk_post = 2;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_post', 1, 'value -> NULL');

SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_post SET title = 'back' WHERE pk_post = 2;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_post', 1, 'NULL -> value');

SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_user SET name = 'renamed' WHERE pk_user = 3;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_user', 1, 'cascade real change (own)');
SELECT _expect('tv_post', 20, 'cascade real change (parents)');

SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_item SET label = 'relabelled' WHERE pk_item = 1;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_item', 1, 'direct-patch real change');

SET pg_tviews.direct_patch_enabled = off;
SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_article SET title = 'retitled' WHERE pk_article = 2;
UPDATE tb_item SET label = 'relabelled' WHERE pk_item = 2;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_article', 1, 'smart-patch real change');
SELECT _expect('tv_item', 1, 'full-replacement real change');
SET pg_tviews.direct_patch_enabled = on;

SELECT pg_stat_force_next_flush();
SELECT _set_mark();
UPDATE tb_contract SET status = 'signed' WHERE id_contract = 100 AND version_no = 2;
SELECT pg_stat_force_next_flush();
SELECT _expect('tv_contract', 1, 'distinct-on real change');

-- ── every tview still matches its backing view ──────────────────────────────
DO $$
DECLARE r text;
BEGIN
    FOREACH r IN ARRAY ARRAY['user', 'post', 'article', 'item', 'contract'] LOOP
        EXECUTE format(
            'DO $d$ BEGIN IF EXISTS (SELECT 1 FROM tv_%1$s t FULL JOIN v_%1$s v USING (pk_%1$s) '
            'WHERE t.data IS DISTINCT FROM v.data) THEN '
            'RAISE EXCEPTION ''FAIL #72: tv_%1$s diverges from v_%1$s''; END IF; END $d$', r);
    END LOOP;
END $$;

\echo 'PASS regress_issue_72_noop_refresh'
