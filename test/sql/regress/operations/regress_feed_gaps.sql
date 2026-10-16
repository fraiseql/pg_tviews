-- Regression test (#247): tviews.pg_tviews_feed_gaps() counts the transactions
-- committed without publishing their changes, for every session.
--
-- Above 90 % NOTIFY queue usage a transaction commits unpublished. A listener
-- that stays connected learns it missed one only from a counter it can read
-- itself: monotonic, server-wide, readable by any role. Filling the queue is
-- covered by test/isolation (feed-gaps); here, the function's contract.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_feed_gaps.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'feed gaps FAIL: %', what; END IF; END $$;

SELECT must(pg_typeof(tviews.pg_tviews_feed_gaps()) = 'bigint'::regtype, 'returns bigint');
SELECT tviews.pg_tviews_feed_gaps() AS before \gset
SELECT must(:before >= 0, 'negative: ' || :before);

-- A transaction published normally is no gap.
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text NOT NULL);
SELECT tviews.pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, t.id, jsonb_build_object('name', t.name) AS data FROM tb_tag t $$);
SET pg_tviews.publish_changes = rows;
INSERT INTO tb_tag (pk_tag, name) VALUES (1, 'a');
SELECT must(tviews.pg_tviews_feed_gaps() = :before, 'a published transaction counted');

-- Any role can read it.
DROP ROLE IF EXISTS feed_gaps_reader;
CREATE ROLE feed_gaps_reader;
SET ROLE feed_gaps_reader;
SELECT must(tviews.pg_tviews_feed_gaps() >= :before, 'read by an unprivileged role');
RESET ROLE;
DROP ROLE feed_gaps_reader;

-- The health report shows it.
SELECT must((tviews.pg_tviews_queue_stats()) ? 'feed_gaps', 'queue stats: '
            || tviews.pg_tviews_queue_stats()::text);

\echo 'regress_feed_gaps: PASS'
