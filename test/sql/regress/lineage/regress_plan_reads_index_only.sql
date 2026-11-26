-- The planner check of the read sets (test-hooks builds) reads an index-only
-- scan's output as its index's columns, not as columns the definition reads: a
-- table joined only to count its rows, scanned through an index on (fk_maker,
-- slug), outputs slug although nothing reads it.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_plan_reads_index_only.sql
--
-- expect-output: plan reads, index-only scan: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

CREATE TABLE tb_maker (pk_maker int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       name text NOT NULL);
CREATE TABLE tb_model (pk_model int PRIMARY KEY, fk_maker int NOT NULL REFERENCES tb_maker,
                       slug text NOT NULL, note text);
CREATE INDEX tb_model_maker_slug ON tb_model (fk_maker, slug);
INSERT INTO tb_maker SELECT g, gen_random_uuid(), 'm' || g FROM generate_series(1, 20) g;
-- Wide rows: the index is the cheaper way to read fk_maker (the owner frame plans
-- with the server's planner settings, so the session cannot force it).
INSERT INTO tb_model SELECT g, 1 + g % 20, 's' || g, repeat('n', 1500) FROM generate_series(1, 4000) g;
VACUUM ANALYZE tb_maker, tb_model;

\set def 'SELECT m.pk_maker, m.id, jsonb_build_object(''name'', m.name, ''models'', count(x.fk_maker)) AS data FROM tb_maker m LEFT JOIN tb_model x ON x.fk_maker = m.pk_maker GROUP BY m.pk_maker, m.id, m.name'

-- The case holds: tb_model is read by an index-only scan outputting slug.
CREATE FUNCTION explained(q text) RETURNS SETOF text LANGUAGE plpgsql AS $$
BEGIN RETURN QUERY EXECUTE 'EXPLAIN (VERBOSE, COSTS OFF) ' || q; END $$;
SELECT must(position('x.slug' IN plan) > 0 AND position('Index Only Scan' IN plan) > 0,
            'the plan reads tb_model by an index-only scan outputting slug: ' || plan)
FROM (SELECT string_agg(line, E'\n') AS plan FROM explained(:'def') line) e;

SELECT tviews.pg_tviews_create('tv_maker', :'def') IS NOT NULL AS created;
UPDATE tb_model SET slug = slug || '+' WHERE pk_model = 1;
SELECT assert_fresh('tv_maker', 'pk_maker', 'an UPDATE of the unread slug');
INSERT INTO tb_model VALUES (5000, 3, 'new', NULL);
SELECT assert_fresh('tv_maker', 'pk_maker', 'an INSERT counted');

SELECT 'plan reads, index-only scan: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
