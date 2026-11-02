-- The jsonb_delta array helpers pg_tviews is installed with behave as their
-- callers expect: jsonb_extract_id, jsonb_array_contains_id,
-- jsonb_array_insert_where, jsonb_delta_array_update_where_path (a path inside
-- the matched element) and jsonb_array_update_where_batch (several elements,
-- some untouched, an empty batch a no-op).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/direct_patch/regress_jsonb_delta_helpers.sql
-- expect-output: jsonb_delta_helpers: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'jsonb_delta_helpers FAIL: %', what; END IF; END $$;

-- jsonb_extract_id / jsonb_array_contains_id.
SELECT must(jsonb_extract_id('{"id": "user_123", "name": "Alice"}', 'id') = 'user_123', 'extract id');
SELECT must(jsonb_extract_id('{"uuid": "abc-def-ghi", "name": "Bob"}', 'uuid') = 'abc-def-ghi', 'extract custom key');
SELECT must(jsonb_array_contains_id('{"comments": [{"id": 1, "text": "Hello"}, {"id": 2, "text": "World"}]}',
                                    'comments', 'id', '2'), 'contains id 2');
SELECT must(NOT jsonb_array_contains_id('{"comments": [{"id": 1, "text": "Hello"}, {"id": 2, "text": "World"}]}',
                                        'comments', 'id', '99'), 'does not contain id 99');

-- jsonb_array_insert_where, then the duplicate it would make detected.
SELECT must(jsonb_array_length(jsonb_array_insert_where('{"items": []}', 'items',
                                                        '{"id": 1, "name": "Item 1"}', NULL, NULL) -> 'items') = 1,
            'insert into an empty array');
SELECT must(jsonb_array_contains_id(jsonb_array_insert_where('{"items": []}', 'items',
                                                             '{"id": 1, "name": "Item 1"}', NULL, NULL),
                                    'items', 'id', '1'), 'inserted element not found');

-- jsonb_delta_array_update_where_path.
CREATE TABLE nested (pk int PRIMARY KEY, data jsonb);
INSERT INTO nested VALUES (1, '{"items": [{"id": 1, "metadata": {"tags": [{"name": "urgent", "color": "red"}]}},
                                         {"id": 2, "metadata": {"tags": [{"name": "normal", "color": "blue"}]}}]}');
UPDATE nested SET data = jsonb_delta_array_update_where_path(data, 'items', 'id', '1', 'metadata.tags[0].color', '"green"');
SELECT must((SELECT data #>> '{items,0,metadata,tags,0,color}' FROM nested) = 'green', 'deep path update');
UPDATE nested SET data = jsonb_delta_array_update_where_path(data, 'items', 'id', '2', 'metadata.tags[0].name', '"important"');
SELECT must((SELECT data #>> '{items,0,metadata,tags,0,color}' = 'green'
                AND data #>> '{items,1,metadata,tags,0,name}' = 'important' FROM nested),
            'a second path update lost the first');

-- jsonb_array_update_where_batch.
CREATE TABLE batch (pk int PRIMARY KEY, data jsonb);
INSERT INTO batch VALUES (1, '{"items": [{"id": 1, "name": "Product A", "price": 10.99},
                                        {"id": 2, "name": "Product B", "price": 15.99},
                                        {"id": 3, "name": "Product C", "price": 20.99}]}');
UPDATE batch SET data = jsonb_array_update_where_batch(data, 'items', 'id', '[
    {"match_value": 1, "updates": {"price": 12.99, "name": "Updated Product A"}},
    {"match_value": 2, "updates": {"price": 17.99}},
    {"match_value": 3, "updates": {"name": "Updated Product C", "stock": 50}}]');
SELECT must((SELECT (data #>> '{items,0,price}')::numeric = 12.99 AND data #>> '{items,0,name}' = 'Updated Product A'
                AND (data #>> '{items,1,price}')::numeric = 17.99 AND data #>> '{items,2,name}' = 'Updated Product C'
                AND (data #>> '{items,2,stock}')::int = 50 FROM batch), 'batch updates');
UPDATE batch SET data = jsonb_array_update_where_batch(data, 'items', 'id', '[
    {"match_value": 1, "updates": {"category": "Electronics"}},
    {"match_value": 3, "updates": {"category": "Books"}}]');
SELECT must((SELECT data #>> '{items,0,category}' = 'Electronics' AND data #>> '{items,1,category}' IS NULL
                AND data #>> '{items,2,category}' = 'Books' FROM batch), 'partial batch');
SELECT must((SELECT jsonb_array_update_where_batch(data, 'items', 'id', '[]') = data FROM batch), 'empty batch changed the document');

\echo 'jsonb_delta_helpers: PASS'
