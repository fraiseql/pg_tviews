-- Benchmark schema for testing cascade performance
-- Simulates a blog with: authors → posts → comments (3-level cascade)

-- 1. Source tables (normalized data)
CREATE TABLE bench_authors (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE bench_posts (
    id SERIAL PRIMARY KEY,
    author_id INTEGER NOT NULL REFERENCES bench_authors(id),
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    status TEXT DEFAULT 'draft',
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE bench_comments (
    id SERIAL PRIMARY KEY,
    post_id INTEGER NOT NULL REFERENCES bench_posts(id),
    author_id INTEGER NOT NULL REFERENCES bench_authors(id),
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- 2. Create indexes for performance
CREATE INDEX idx_posts_author ON bench_posts(author_id);
CREATE INDEX idx_comments_post ON bench_comments(post_id);
CREATE INDEX idx_comments_author ON bench_comments(author_id);

-- 3. Denormalized views (what gets transformed to JSONB)
CREATE VIEW v_bench_comments AS
SELECT
    c.id,
    c.post_id,
    jsonb_build_object(
        'id', c.id,
        'content', c.content,
        'created_at', c.created_at,
        'author', jsonb_build_object(
            'id', a.id,
            'name', a.name,
            'email', a.email
        )
    ) AS data
FROM bench_comments c
JOIN bench_authors a ON c.author_id = a.id;

CREATE VIEW v_bench_posts AS
SELECT
    p.id,
    p.author_id,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'content', p.content,
        'status', p.status,
        'created_at', p.created_at,
        'author', jsonb_build_object(
            'id', a.id,
            'name', a.name,
            'email', a.email
        ),
        'comments', COALESCE(
            (SELECT jsonb_agg(
                jsonb_build_object(
                    'id', c.id,
                    'content', c.content,
                    'created_at', c.created_at,
                    'author', jsonb_build_object(
                        'id', ca.id,
                        'name', ca.name,
                        'email', ca.email
                    )
                )
            )
            FROM bench_comments c
            JOIN bench_authors ca ON c.author_id = ca.id
            WHERE c.post_id = p.id),
            '[]'::jsonb
        )
    ) AS data
FROM bench_posts p
JOIN bench_authors a ON p.author_id = a.id;

-- 4. TVIEW tables (materialized JSONB)
CREATE TABLE tv_bench_comments (
    id INTEGER PRIMARY KEY,
    post_id INTEGER NOT NULL,
    data JSONB NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE tv_bench_posts (
    id INTEGER PRIMARY KEY,
    author_id INTEGER NOT NULL,
    data JSONB NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT now()
);

-- 5. Insert metadata for smart patching
-- This tells pg_tviews how to patch each dependency

-- For tv_bench_comments: has nested author object
INSERT INTO pg_tview_meta (
    tview_oid,
    source_table_names,
    fk_columns,
    dependency_types,
    dependency_paths,
    array_match_keys
) VALUES (
    'tv_bench_comments'::regclass::oid,
    ARRAY['bench_authors'],
    ARRAY['author_id'],
    ARRAY['nested_object'],
    ARRAY['author'],  -- Flat TEXT[] for path
    ARRAY[NULL]       -- No match key for nested objects
);

-- For tv_bench_posts: has nested author + array of comments
INSERT INTO pg_tview_meta (
    tview_oid,
    source_table_names,
    fk_columns,
    dependency_types,
    dependency_paths,
    array_match_keys
) VALUES (
    'tv_bench_posts'::regclass::oid,
    ARRAY['bench_authors', 'bench_comments'],
    ARRAY['author_id', NULL],  -- NULL for comments (array, not direct FK)
    ARRAY['nested_object', 'array'],
    ARRAY['author', 'comments'],  -- Flat TEXT[] paths
    ARRAY[NULL, 'id']  -- Match key 'id' for comments array
);

-- 6. Helper function: populate TVIEW from view
CREATE OR REPLACE FUNCTION refresh_tview_comments() RETURNS void
LANGUAGE sql AS $$
    TRUNCATE tv_bench_comments;
    INSERT INTO tv_bench_comments (id, post_id, data)
    SELECT id, post_id, data FROM v_bench_comments;
$$;

CREATE OR REPLACE FUNCTION refresh_tview_posts() RETURNS void
LANGUAGE sql AS $$
    TRUNCATE tv_bench_posts;
    INSERT INTO tv_bench_posts (id, author_id, data)
    SELECT id, author_id, data FROM v_bench_posts;
$$;