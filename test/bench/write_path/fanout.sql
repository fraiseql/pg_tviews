-- Fan-out cost bench (#233): one category embedded in N items. Variables:
--   fanouts    comma-separated fan-outs, e.g. '1,10,100,1000,10000'
--   doc_bytes  item document size: ~1000 inline; above 2000 the content is incompressible
--              hex, so the documents are stored out of line (TOASTed)
-- Categories per fan-out: 20, fewer for large ones (at most 100,000 items per fan-out,
-- at least 5 categories), so the dataset grows with the largest N asked for.
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE SCHEMA fan;
SET search_path = fan, public;
CREATE TABLE tb_cat (
    pk_cat int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fanout int NOT NULL,
    name text NOT NULL
);
CREATE TABLE tb_item (
    pk_item int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_cat int NOT NULL REFERENCES tb_cat,
    title text NOT NULL,
    content text NOT NULL
);
CREATE INDEX ON tb_item (fk_cat);
SELECT tviews.pg_tviews_create('fan.tv_cat', $$
    SELECT c.pk_cat, c.id, jsonb_build_object('id', c.id::text, 'name', c.name, 'fanout', c.fanout) AS data
    FROM fan.tb_cat c $$);
SELECT tviews.pg_tviews_create('fan.tv_item', $$
    SELECT i.pk_item, i.id, i.fk_cat,
           jsonb_build_object('id', i.id::text, 'title', i.title, 'content', i.content,
               'category', jsonb_build_object('id', c.id::text, 'name', c.name)) AS data
    FROM fan.tb_item i JOIN fan.tb_cat c ON c.pk_cat = i.fk_cat $$);
INSERT INTO tb_cat (fanout, name)
SELECT f, 'cat ' || f || '/' || g
FROM unnest(string_to_array(:'fanouts', ',')::int[]) f,
     generate_series(1, least(20, greatest(5, 100000 / f))) g
ORDER BY f, g;
INSERT INTO tb_item (fk_cat, title, content)
SELECT c.pk_cat, 'Item ' || c.pk_cat || '/' || g,
       CASE WHEN :doc_bytes > 2000
            THEN (SELECT string_agg(md5(random()::text || x), '') FROM generate_series(1, :doc_bytes / 32) x WHERE g > 0)
            ELSE repeat('content words ', :doc_bytes / 14) END
FROM tb_cat c, generate_series(1, c.fanout) g;
VACUUM ANALYZE;
