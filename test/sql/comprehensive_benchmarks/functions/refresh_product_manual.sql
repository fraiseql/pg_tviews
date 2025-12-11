-- Generic Refresh Function for Manual Function Approach
-- Approach 3: Manual refresh with unlimited cascade depth and maximum optimization

CREATE OR REPLACE FUNCTION refresh_product_manual(
    p_entity_type TEXT,           -- 'product', 'category', 'supplier', 'inventory', 'review'
    p_entity_pk INTEGER,          -- Primary key of changed entity
    p_change_type TEXT DEFAULT 'full_update',  -- Specific field hint for optimization
    p_max_retries INTEGER DEFAULT 3  -- For optimistic concurrency
)
RETURNS JSONB AS $$
DECLARE
    v_start_time TIMESTAMPTZ;
    v_end_time TIMESTAMPTZ;
    v_execution_ms NUMERIC;
    v_products_refreshed INTEGER := 0;
    v_cascades_triggered INTEGER := 0;
    v_retry_count INTEGER := 0;
    v_success BOOLEAN := false;
    v_result JSONB;
BEGIN
    v_start_time := clock_timestamp();

    -- Input validation
    IF p_entity_type NOT IN ('product', 'category', 'supplier', 'inventory', 'review') THEN
        RAISE EXCEPTION 'Invalid entity_type: %. Must be one of: product, category, supplier, inventory, review', p_entity_type;
    END IF;

    IF p_entity_pk IS NULL OR p_entity_pk <= 0 THEN
        RAISE EXCEPTION 'Invalid entity_pk: %. Must be a positive integer', p_entity_pk;
    END IF;

    -- Main refresh logic with retry mechanism
    WHILE v_retry_count < p_max_retries AND NOT v_success LOOP
        BEGIN
            -- Execute the appropriate refresh logic based on entity type
            CASE p_entity_type
                WHEN 'product' THEN
                    PERFORM refresh_product_entity(p_entity_pk, p_change_type);
                    v_products_refreshed := 1;
                    v_cascades_triggered := 0;

                WHEN 'category' THEN
                    SELECT * INTO v_products_refreshed, v_cascades_triggered
                    FROM refresh_category_cascade(p_entity_pk, p_change_type);

                WHEN 'supplier' THEN
                    SELECT * INTO v_products_refreshed, v_cascades_triggered
                    FROM refresh_supplier_cascade(p_entity_pk, p_change_type);

                WHEN 'inventory' THEN
                    PERFORM refresh_inventory_cascade(p_entity_pk, p_change_type);
                    v_products_refreshed := 1;
                    v_cascades_triggered := 0;

                WHEN 'review' THEN
                    PERFORM refresh_review_cascade(p_entity_pk, p_change_type);
                    v_products_refreshed := 1;
                    v_cascades_triggered := 0;
            END CASE;

            v_success := true;

        EXCEPTION
            WHEN serialization_failure OR deadlock_detected THEN
                -- Optimistic concurrency failure, retry
                v_retry_count := v_retry_count + 1;
                IF v_retry_count < p_max_retries THEN
                    -- Exponential backoff: wait 10ms, 20ms, 40ms...
                    PERFORM pg_sleep(0.01 * power(2, v_retry_count - 1));
                    CONTINUE;
                ELSE
                    -- Final failure
                    RAISE EXCEPTION 'Refresh failed after % retries due to concurrency conflicts', p_max_retries;
                END IF;
            WHEN OTHERS THEN
                -- Re-raise other exceptions
                RAISE;
        END;
    END LOOP;

    v_end_time := clock_timestamp();
    v_execution_ms := EXTRACT(EPOCH FROM (v_end_time - v_start_time)) * 1000;

    -- Build result statistics
    v_result := jsonb_build_object(
        'products_refreshed', v_products_refreshed,
        'cascades_triggered', v_cascades_triggered,
        'execution_ms', ROUND(v_execution_ms, 3),
        'change_type', p_change_type,
        'entity_type', p_entity_type,
        'entity_pk', p_entity_pk,
        'retries_used', v_retry_count
    );

    RETURN v_result;

EXCEPTION
    WHEN OTHERS THEN
        -- Return error information
        v_end_time := clock_timestamp();
        v_execution_ms := EXTRACT(EPOCH FROM (v_end_time - v_start_time)) * 1000;

        RETURN jsonb_build_object(
            'error', SQLERRM,
            'execution_ms', ROUND(v_execution_ms, 3),
            'change_type', p_change_type,
            'entity_type', p_entity_type,
            'entity_pk', p_entity_pk,
            'retries_used', v_retry_count
        );
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- Helper Functions for Different Entity Types
-- ============================================================================

-- Single product refresh with surgical updates
CREATE OR REPLACE FUNCTION refresh_product_entity(
    p_product_pk INTEGER,
    p_change_type TEXT DEFAULT 'full_update'
)
RETURNS void AS $$
DECLARE
    v_current_data JSONB;
    v_new_data JSONB;
    v_old_version INTEGER;
BEGIN
    -- Get current data and version for optimistic concurrency
    SELECT data, version INTO v_current_data, v_old_version
    FROM manual_func_product
    WHERE pk_product = p_product_pk;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Product % not found in manual_func_product', p_product_pk;
    END IF;

    -- Build new data based on change type for surgical updates
    CASE p_change_type
        WHEN 'price_current' THEN
            -- Update only current price and discount_pct
            SELECT
                jsonb_set(
                    jsonb_set(
                        v_current_data,
                        '{price,current}',
                        to_jsonb(p.current_price)
                    ),
                    '{price,discount_pct}',
                    to_jsonb(ROUND((1 - p.current_price / NULLIF(p.base_price, 0)) * 100, 2))
                )
            INTO v_new_data
            FROM tb_product p
            WHERE p.pk_product = p_product_pk;

        WHEN 'price_base' THEN
            -- Update base price and recalculate discount_pct
            SELECT
                jsonb_set(
                    jsonb_set(
                        v_current_data,
                        '{price,base}',
                        to_jsonb(p.base_price)
                    ),
                    '{price,discount_pct}',
                    to_jsonb(ROUND((1 - p.current_price / NULLIF(p.base_price, 0)) * 100, 2))
                )
            INTO v_new_data
            FROM tb_product p
            WHERE p.pk_product = p_product_pk;

        WHEN 'status' THEN
            -- Update only status
            SELECT jsonb_set(v_current_data, '{status}', to_jsonb(p.status))
            INTO v_new_data
            FROM tb_product p
            WHERE p.pk_product = p_product_pk;

        WHEN 'full_update' THEN
            -- Full rebuild from v_product view
            SELECT data INTO v_new_data
            FROM v_product
            WHERE pk_product = p_product_pk;

        ELSE
            -- Unknown change type, do full update
            SELECT data INTO v_new_data
            FROM v_product
            WHERE pk_product = p_product_pk;
    END CASE;

    -- Update with optimistic concurrency check
    UPDATE manual_func_product
    SET data = v_new_data,
        version = version + 1,
        updated_at = now()
    WHERE pk_product = p_product_pk
    AND version = v_old_version;

    IF NOT FOUND THEN
        RAISE serialization_failure;
    END IF;

END;
$$ LANGUAGE plpgsql;

-- Category cascade: Update all products in a category
CREATE OR REPLACE FUNCTION refresh_category_cascade(
    p_category_pk INTEGER,
    p_change_type TEXT DEFAULT 'full_update'
)
RETURNS TABLE(products_refreshed INTEGER, cascades_triggered INTEGER) AS $$
DECLARE
    v_category_data JSONB;
    v_product_count INTEGER;
BEGIN
    -- Get updated category data
    SELECT jsonb_build_object(
        'id', id,
        'pk', pk_category,
        'name', name,
        'slug', slug
    ) INTO v_category_data
    FROM tb_category
    WHERE pk_category = p_category_pk;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Category % not found', p_category_pk;
    END IF;

    -- Bulk update all products in this category
    UPDATE manual_func_product mfp
    SET data = jsonb_set(mfp.data, '{category}', v_category_data),
        version = version + 1,
        updated_at = now()
    WHERE mfp.pk_product IN (
        SELECT p.pk_product
        FROM tb_product p
        WHERE p.fk_category = p_category_pk
    );

    GET DIAGNOSTICS v_product_count = ROW_COUNT;

    -- Return statistics
    RETURN QUERY SELECT v_product_count, 1::INTEGER;
END;
$$ LANGUAGE plpgsql;

-- Supplier cascade: Update all products from a supplier
CREATE OR REPLACE FUNCTION refresh_supplier_cascade(
    p_supplier_pk INTEGER,
    p_change_type TEXT DEFAULT 'full_update'
)
RETURNS TABLE(products_refreshed INTEGER, cascades_triggered INTEGER) AS $$
DECLARE
    v_supplier_data JSONB;
    v_product_count INTEGER;
BEGIN
    -- Get updated supplier data (handle NULL suppliers)
    SELECT CASE WHEN s.pk_supplier IS NOT NULL THEN
        jsonb_build_object(
            'id', s.id,
            'pk', s.pk_supplier,
            'name', s.name,
            'email', s.contact_email,
            'country', s.country
        )
    ELSE NULL END
    INTO v_supplier_data
    FROM tb_supplier s
    WHERE s.pk_supplier = p_supplier_pk;

    -- Note: If supplier not found, we still proceed (supplier may have been deleted)

    -- Bulk update all products from this supplier
    UPDATE manual_func_product mfp
    SET data = jsonb_set(mfp.data, '{supplier}', v_supplier_data),
        version = version + 1,
        updated_at = now()
    WHERE mfp.pk_product IN (
        SELECT p.pk_product
        FROM tb_product p
        WHERE p.fk_supplier = p_supplier_pk
    );

    GET DIAGNOSTICS v_product_count = ROW_COUNT;

    -- Return statistics
    RETURN QUERY SELECT v_product_count, 1::INTEGER;
END;
$$ LANGUAGE plpgsql;

-- Inventory cascade: Update single product inventory
CREATE OR REPLACE FUNCTION refresh_inventory_cascade(
    p_inventory_pk INTEGER,
    p_change_type TEXT DEFAULT 'full_update'
)
RETURNS void AS $$
DECLARE
    v_product_pk INTEGER;
    v_inventory_data JSONB;
BEGIN
    -- Get product PK for this inventory
    SELECT fk_product INTO v_product_pk
    FROM tb_inventory
    WHERE pk_inventory = p_inventory_pk;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Inventory % not found', p_inventory_pk;
    END IF;

    -- Get updated inventory data
    SELECT jsonb_build_object(
        'quantity', COALESCE(i.quantity, 0),
        'available', COALESCE(i.quantity - i.reserved, 0),
        'in_stock', COALESCE(i.quantity, 0) > 0,
        'warehouse', i.warehouse_location
    ) INTO v_inventory_data
    FROM tb_inventory i
    WHERE i.pk_inventory = p_inventory_pk;

    -- Update the specific product
    UPDATE manual_func_product
    SET data = jsonb_set(data, '{inventory}', v_inventory_data),
        version = version + 1,
        updated_at = now()
    WHERE pk_product = v_product_pk;

END;
$$ LANGUAGE plpgsql;

-- Review cascade: Update single product with full review recount
CREATE OR REPLACE FUNCTION refresh_review_cascade(
    p_review_pk INTEGER,
    p_change_type TEXT DEFAULT 'full_update'
)
RETURNS void AS $$
DECLARE
    v_product_pk INTEGER;
    v_reviews_data JSONB;
BEGIN
    -- Get product PK for this review
    SELECT fk_product INTO v_product_pk
    FROM tb_review
    WHERE pk_review = p_review_pk;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Review % not found', p_review_pk;
    END IF;

    -- Rebuild complete reviews data (full recount for accuracy)
    SELECT jsonb_build_object(
        'count', COUNT(r.*)::INTEGER,
        'average_rating', ROUND(AVG(r.rating), 2),
        'recent', COALESCE(
            jsonb_agg(
                jsonb_build_object(
                    'id', r.id,
                    'pk', r.pk_review,
                    'rating', r.rating,
                    'title', r.title,
                    'verified', r.verified_purchase,
                    'created_at', r.created_at
                ) ORDER BY r.created_at DESC
            ) FILTER (WHERE r.pk_review IS NOT NULL),
            '[]'::jsonb
        )
    ) INTO v_reviews_data
    FROM tb_review r
    WHERE r.fk_product = v_product_pk;

    -- Update the specific product
    UPDATE manual_func_product
    SET data = jsonb_set(data, '{reviews}', v_reviews_data),
        version = version + 1,
        updated_at = now()
    WHERE pk_product = v_product_pk;

END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION refresh_product_manual(TEXT, INTEGER, TEXT, INTEGER) IS 'Generic refresh function for Approach 3: Manual function with unlimited cascade support';
COMMENT ON FUNCTION refresh_product_entity(INTEGER, TEXT) IS 'Refresh single product with surgical JSONB updates';
COMMENT ON FUNCTION refresh_category_cascade(INTEGER, TEXT) IS 'Cascade refresh all products in a category';
COMMENT ON FUNCTION refresh_supplier_cascade(INTEGER, TEXT) IS 'Cascade refresh all products from a supplier';
COMMENT ON FUNCTION refresh_inventory_cascade(INTEGER, TEXT) IS 'Refresh single product inventory data';
COMMENT ON FUNCTION refresh_review_cascade(INTEGER, TEXT) IS 'Refresh single product with full review recount';