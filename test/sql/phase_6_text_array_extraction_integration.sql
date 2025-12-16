-- Phase 6 Integration Tests: TEXT[][] Extraction Workaround
-- Tests dependency_paths extraction and nested JSONB refresh functionality

-- Test 1: Metadata with dependency paths
-- Create tables with relationships
CREATE TABLE tb_author (
    pk_author BIGINT PRIMARY KEY,
    name TEXT
);

CREATE TABLE tb_book (
    pk_book BIGINT PRIMARY KEY,
    fk_author BIGINT,
    title TEXT
);

CREATE TABLE tb_review (
    pk_review BIGINT PRIMARY KEY,
    fk_book BIGINT,
    rating INTEGER,
    comment TEXT
);

-- Create TVIEWs with nested relationships
SELECT pg_tviews_create('author', $$
    SELECT pk_author,
           jsonb_build_object('name', name) as data
    FROM tb_author
$$);

SELECT pg_tviews_create('book', $$
    SELECT b.pk_book,
           jsonb_build_object(
               'fk_author', b.fk_author,
               'title', b.title,
               'author', jsonb_build_object(
                   'name', a.name
               )
           ) as data
    FROM tb_book b
    LEFT JOIN tb_author a ON b.fk_author = a.pk_author
$$);

SELECT pg_tviews_create('review', $$
    SELECT r.pk_review,
           jsonb_build_object(
               'fk_book', r.fk_book,
               'rating', r.rating,
               'comment', r.comment,
               'book', jsonb_build_object(
                   'title', b.title,
                   'author', jsonb_build_object(
                       'name', a.name
                   )
               )
           ) as data
    FROM tb_review r
    LEFT JOIN tb_book b ON r.fk_book = b.pk_book
    LEFT JOIN tb_author a ON b.fk_author = a.pk_author
$$);

-- Insert test data
INSERT INTO tb_author VALUES (1, 'Stephen King'), (2, 'J.K. Rowling');
INSERT INTO tb_book VALUES (1, 1, 'The Shining'), (2, 2, 'Harry Potter');
INSERT INTO tb_review VALUES (1, 1, 5, 'Scary!'), (2, 2, 4, 'Magical!');

-- Test 2: Verify dependency_paths extraction
-- The metadata should now include proper dependency paths
SELECT entity, dependency_paths
FROM pg_tview_meta
WHERE entity IN ('author', 'book', 'review')
ORDER BY entity;

-- Test 3: Nested refresh operations
BEGIN;
    -- Update author (should cascade to books and reviews)
    UPDATE tb_author SET name = 'Stephen King (Updated)' WHERE pk_author = 1;

    -- Check queue shows cascade
    SELECT * FROM pg_tviews_queue_info();
    -- Should show author, book, review entities

COMMIT;

-- Verify nested data was updated
SELECT
    a.data->>'name' as author_name,
    b.data->>'title' as book_title,
    b.data->'author'->>'name' as book_author_name,
    r.data->'book'->>'title' as review_book_title,
    r.data->'book'->'author'->>'name' as review_author_name
FROM tv_author a
JOIN tv_book b ON (b.data->>'fk_author')::bigint = (a.data->>'id')::bigint
JOIN tv_review r ON (r.data->>'fk_book')::bigint = (b.data->>'id')::bigint;

-- Test 4: Complex nested updates
BEGIN;
    -- Update book title (should cascade to reviews)
    UPDATE tb_book SET title = 'The Shining (Updated)' WHERE pk_book = 1;

    -- Update review (should only affect review)
    UPDATE tb_review SET comment = 'Still scary!' WHERE pk_review = 1;

COMMIT;

-- Verify book title updated in reviews
SELECT r.data->'book'->>'title' as nested_book_title
FROM tv_review r
WHERE (r.data->>'id')::bigint = 1;

-- Test 5: Array-based nested operations
-- Create tables with array relationships
CREATE TABLE tb_category (
    pk_category BIGINT PRIMARY KEY,
    name TEXT
);

CREATE TABLE tb_product (
    pk_product BIGINT PRIMARY KEY,
    name TEXT,
    category_ids BIGINT[]
);

-- Add category references to products via array
INSERT INTO tb_category VALUES (1, 'Books'), (2, 'Electronics');
INSERT INTO tb_product VALUES (1, 'Kindle', ARRAY[1, 2]);

-- Test array path updates work with dependency_paths
SELECT pg_tviews_create('category', $$
    SELECT pk_category,
           jsonb_build_object('name', name) as data
    FROM tb_category
$$);

SELECT pg_tviews_create('product', $$
    SELECT p.pk_product,
           jsonb_build_object(
               'name', p.name,
               'categories', (
                   SELECT jsonb_agg(
                       jsonb_build_object('name', c.name)
                   )
                   FROM tb_category c
                   WHERE c.pk_category = ANY(p.category_ids)
               )
           ) as data
    FROM tb_product p
$$);

BEGIN;
    -- Update category (should cascade to products via array)
    UPDATE tb_category SET name = 'Books & Media' WHERE pk_category = 1;

    -- Check cascade worked
    SELECT p.data->'categories' as nested_categories
    FROM tv_product p
    WHERE (p.data->>'id')::bigint = 1;

COMMIT;

-- Test 6: Error handling for malformed dependency_paths
-- The workaround should handle cases where dependency_paths is NULL or malformed
SELECT entity, dependency_paths
FROM pg_tview_meta
WHERE dependency_paths IS NULL OR dependency_paths = '{}';

-- Cleanup
DROP TABLE tb_author CASCADE;
DROP TABLE tb_book CASCADE;
DROP TABLE tb_review CASCADE;
DROP TABLE tb_category CASCADE;
DROP TABLE tb_product CASCADE;
SELECT pg_tviews_drop('author');
SELECT pg_tviews_drop('book');
SELECT pg_tviews_drop('review');
SELECT pg_tviews_drop('category');
SELECT pg_tviews_drop('product');