-- E-Commerce Product Catalog Schema
-- Realistic schema: categories → products → reviews → inventory
-- Uses trinity pattern: id (UUID) + pk_{entity} (INTEGER) + fk_{entity} (INTEGER)

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- Source tables (command side with trinity pattern)
CREATE TABLE tb_category (
    id UUID DEFAULT uuid_generate_v4(),
    pk_category SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,
    fk_parent_category INTEGER REFERENCES tb_category(pk_category),
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE tb_supplier (
    id UUID DEFAULT uuid_generate_v4(),
    pk_supplier SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    contact_email TEXT,
    contact_phone TEXT,
    country TEXT,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE tb_product (
    id UUID DEFAULT uuid_generate_v4(),
    pk_product SERIAL PRIMARY KEY,
    fk_category INTEGER NOT NULL REFERENCES tb_category(pk_category),
    fk_supplier INTEGER REFERENCES tb_supplier(pk_supplier),
    sku TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT,
    base_price NUMERIC(10, 2) NOT NULL,
    current_price NUMERIC(10, 2) NOT NULL,
    currency TEXT DEFAULT 'USD',
    status TEXT DEFAULT 'active',
    created_at TIMESTAMPTZ DEFAULT now(),
    updated_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE tb_review (
    id UUID DEFAULT uuid_generate_v4(),
    pk_review SERIAL PRIMARY KEY,
    fk_product INTEGER NOT NULL REFERENCES tb_product(pk_product),
    fk_user INTEGER NOT NULL,
    rating INTEGER CHECK (rating BETWEEN 1 AND 5),
    title TEXT,
    content TEXT,
    verified_purchase BOOLEAN DEFAULT false,
    helpful_count INTEGER DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE tb_inventory (
    id UUID DEFAULT uuid_generate_v4(),
    pk_inventory SERIAL PRIMARY KEY,
    fk_product INTEGER NOT NULL REFERENCES tb_product(pk_product) UNIQUE,
    quantity INTEGER DEFAULT 0,
    reserved INTEGER DEFAULT 0,
    warehouse_location TEXT,
    last_restocked_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ DEFAULT now()
);

-- Indexes for performance
CREATE INDEX idx_category_id ON tb_category(id);
CREATE INDEX idx_supplier_id ON tb_supplier(id);
CREATE INDEX idx_product_id ON tb_product(id);
CREATE INDEX idx_product_category ON tb_product(fk_category);
CREATE INDEX idx_product_supplier ON tb_product(fk_supplier);
CREATE INDEX idx_product_status ON tb_product(status) WHERE status = 'active';
CREATE INDEX idx_review_id ON tb_review(id);
CREATE INDEX idx_review_product ON tb_review(fk_product);
CREATE INDEX idx_review_rating ON tb_review(rating);
CREATE INDEX idx_inventory_id ON tb_inventory(id);
CREATE INDEX idx_inventory_product ON tb_inventory(fk_product);

-- Backing view for denormalized data
CREATE VIEW v_product AS
SELECT
    p.pk_product,
    p.fk_category,
    jsonb_build_object(
        'id', p.id,  -- UUID
        'pk', p.pk_product,  -- INTEGER
        'sku', p.sku,
        'name', p.name,
        'description', p.description,
        'price', jsonb_build_object(
            'base', p.base_price,
            'current', p.current_price,
            'currency', p.currency,
            'discount_pct', ROUND((1 - p.current_price / NULLIF(p.base_price, 0)) * 100, 2)
        ),
        'status', p.status,
        'category', jsonb_build_object(
            'id', c.id,
            'pk', c.pk_category,
            'name', c.name,
            'slug', c.slug
        ),
        'supplier', CASE WHEN s.pk_supplier IS NOT NULL THEN
            jsonb_build_object(
                'id', s.id,
                'pk', s.pk_supplier,
                'name', s.name,
                'email', s.contact_email,
                'country', s.country
            )
        ELSE NULL END,
        'inventory', jsonb_build_object(
            'quantity', COALESCE(i.quantity, 0),
            'available', COALESCE(i.quantity - i.reserved, 0),
            'in_stock', COALESCE(i.quantity, 0) > 0,
            'warehouse', i.warehouse_location
        ),
        'reviews', jsonb_build_object(
            'count', (SELECT COUNT(*) FROM tb_review r WHERE r.fk_product = p.pk_product),
            'average_rating', ROUND((SELECT AVG(rating) FROM tb_review r WHERE r.fk_product = p.pk_product), 2),
            'recent', COALESCE(
                (SELECT jsonb_agg(
                    jsonb_build_object(
                        'id', r.id,
                        'pk', r.pk_review,
                        'rating', r.rating,
                        'title', r.title,
                        'verified', r.verified_purchase,
                        'created_at', r.created_at
                    ) ORDER BY r.created_at DESC
                )
                FROM (
                    SELECT * FROM tb_review r
                    WHERE r.fk_product = p.pk_product
                    ORDER BY r.created_at DESC
                    LIMIT 5
                ) r),
                '[]'::jsonb
            )
        ),
        'created_at', p.created_at,
        'updated_at', p.updated_at
    ) AS data
FROM tb_product p
JOIN tb_category c ON p.fk_category = c.pk_category
LEFT JOIN tb_supplier s ON p.fk_supplier = s.pk_supplier
LEFT JOIN tb_inventory i ON p.pk_product = i.fk_product;

-- TVIEW table for materialized product data (projection side)
-- Approach 1: pg_tviews with automatic incremental refresh
CREATE TABLE tv_product (
    pk_product INTEGER PRIMARY KEY,
    fk_category INTEGER NOT NULL,
    data JSONB NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT now()
);

-- GIN index for JSONB queries
CREATE INDEX idx_tv_product_data ON tv_product USING GIN (data);
CREATE INDEX idx_tv_product_category ON tv_product(fk_category);
CREATE INDEX idx_tv_product_id ON tv_product((data->>'id'));  -- Index on UUID in JSONB

-- Manual JSONB table for comparison
-- Approach 2: Manual incremental updates using native PostgreSQL JSONB functions
CREATE TABLE manual_product (
    pk_product INTEGER PRIMARY KEY,
    fk_category INTEGER NOT NULL,
    data JSONB NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT now()
);

-- GIN index for JSONB queries
CREATE INDEX idx_manual_product_data ON manual_product USING GIN (data);
CREATE INDEX idx_manual_product_category ON manual_product(fk_category);
CREATE INDEX idx_manual_product_id ON manual_product((data->>'id'));

-- Manual Function table for comparison
-- Approach 3: Generic refresh function with unlimited cascade support
CREATE TABLE manual_func_product (
    pk_product INTEGER PRIMARY KEY,
    fk_category INTEGER NOT NULL,
    data JSONB NOT NULL,
    version INTEGER DEFAULT 1,  -- For optimistic concurrency control
    updated_at TIMESTAMPTZ DEFAULT now()
);

-- GIN index for JSONB queries
CREATE INDEX idx_manual_func_product_data ON manual_func_product USING GIN (data);
CREATE INDEX idx_manual_func_product_category ON manual_func_product(fk_category);
CREATE INDEX idx_manual_func_product_id ON manual_func_product((data->>'id'));
CREATE INDEX idx_manual_func_product_version ON manual_func_product(version);

-- Note: pg_tview_meta insertion commented out for benchmark simulation
-- In production, pg_tviews would automatically populate this metadata
-- INSERT INTO pg_tview_meta (...) VALUES (...)

-- Traditional materialized view for comparison
-- Approach 4: Full refresh (traditional approach)
CREATE MATERIALIZED VIEW mv_product AS
SELECT * FROM v_product;

CREATE UNIQUE INDEX idx_mv_product_pk ON mv_product(pk_product);
CREATE INDEX idx_mv_product_data ON mv_product USING GIN (data);

-- Helper functions
CREATE OR REPLACE FUNCTION refresh_tv_product() RETURNS void AS $$
BEGIN
    TRUNCATE tv_product;
    INSERT INTO tv_product (pk_product, fk_category, data)
    SELECT pk_product, fk_category, data FROM v_product;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION refresh_manual_product() RETURNS void AS $$
BEGIN
    TRUNCATE manual_product;
    INSERT INTO manual_product (pk_product, fk_category, data)
    SELECT pk_product, fk_category, data FROM v_product;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION refresh_manual_func_product() RETURNS void AS $$
BEGIN
    TRUNCATE manual_func_product;
    INSERT INTO manual_func_product (pk_product, fk_category, data)
    SELECT pk_product, fk_category, data FROM v_product;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION product_counts() RETURNS TABLE(
    total_products BIGINT,
    active_products BIGINT,
    total_reviews BIGINT,
    avg_reviews_per_product NUMERIC
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        COUNT(*)::BIGINT,
        COUNT(*) FILTER (WHERE status = 'active')::BIGINT,
        (SELECT COUNT(*) FROM tb_review)::BIGINT,
        ROUND((SELECT COUNT(*)::NUMERIC FROM tb_review) / NULLIF(COUNT(*), 0), 2)
    FROM tb_product;
END;
$$ LANGUAGE plpgsql;

COMMENT ON TABLE tv_product IS 'Approach 1: pg_tviews with jsonb_ivm optimization';
COMMENT ON TABLE manual_product IS 'Approach 2: Manual incremental updates with native PostgreSQL';
COMMENT ON TABLE manual_func_product IS 'Approach 3: Generic refresh function with unlimited cascade support';
COMMENT ON MATERIALIZED VIEW mv_product IS 'Approach 4: Traditional full REFRESH MATERIALIZED VIEW';
COMMENT ON TABLE tb_category IS 'Command side: Product categories';
COMMENT ON TABLE tb_product IS 'Command side: Product catalog';
COMMENT ON TABLE tb_review IS 'Command side: Product reviews';
COMMENT ON TABLE tb_inventory IS 'Command side: Inventory tracking';
