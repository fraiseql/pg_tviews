-- Regression test for issue #004: name-type OID mismatch
-- The original bug: SPI queries that read 'name'/text columns without explicit
-- ::text casts caused refresh to fail silently, leaving TVIEWs unpopulated after
-- INSERTs that should have triggered a refresh.
--
-- Updated for the current API (issue #55): the removed pg_tviews_register(name,
-- view, table, pk) form is replaced by pg_tviews_create(tview_name, select_sql),
-- which builds the backing view itself. The regression intent is preserved: a
-- TVIEW whose projection carries a text column must populate after an INSERT.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/99-name-oid-regression.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\echo '=========================================='
\echo 'Regression Test: Name-Type OID Mismatch (#004)'
\echo '=========================================='

DROP TABLE IF EXISTS tb_nametest CASCADE;

CREATE TABLE tb_nametest (
    pk_nametest BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id          UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    label       TEXT NOT NULL
);

SELECT pg_tviews_create('tv_nametest', $TVIEW$
    SELECT pk_nametest, id, jsonb_build_object('label', label) AS data
    FROM tb_nametest
$TVIEW$);

-- Insert data (this triggers a refresh).
INSERT INTO tb_nametest (label) VALUES ('hello');

-- Verify the TVIEW is populated and the text column round-trips through refresh.
DO $$
DECLARE
    row_count INTEGER;
    got_label TEXT;
BEGIN
    SELECT COUNT(*) INTO row_count FROM tv_nametest;
    IF row_count <> 1 THEN
        RAISE EXCEPTION '#004 FAIL: tv_nametest should have 1 row after INSERT, has % (refresh dropped by name-type OID mismatch?)', row_count;
    END IF;

    SELECT data->>'label' INTO got_label FROM tv_nametest WHERE pk_nametest = 1;
    IF got_label <> 'hello' THEN
        RAISE EXCEPTION '#004 FAIL: text column did not round-trip through refresh (got %)', got_label;
    END IF;

    RAISE NOTICE '#004 ok: TVIEW populated and text column preserved after INSERT';
END $$;

SELECT pg_tviews_drop('tv_nametest', true);

\echo '#004 PASS: name-type text column refreshes correctly'
