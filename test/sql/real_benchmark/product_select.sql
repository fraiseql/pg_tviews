SELECT
    p.id,
    p.pk_product,
    p.fk_category,
    jsonb_build_object(
        'sku', p.sku,
        'name', p.name,
        'description', p.description,
        'status', p.status,
        'price', jsonb_build_object(
            'base', p.base_price,
            'current', p.current_price,
            'currency', p.currency,
            'discount_pct', ROUND((1 - p.current_price / NULLIF(p.base_price, 0)) * 100, 2)
        ),
        'category', jsonb_build_object('pk', c.pk_category, 'name', c.name, 'slug', c.slug),
        'supplier', CASE WHEN s.pk_supplier IS NOT NULL
            THEN jsonb_build_object('pk', s.pk_supplier, 'name', s.name, 'country', s.country)
            ELSE NULL END,
        'inventory', jsonb_build_object(
            'quantity', COALESCE(i.quantity, 0),
            'reserved', COALESCE(i.reserved, 0),
            'available', COALESCE(i.quantity - i.reserved, 0),
            'in_stock', COALESCE(i.quantity, 0) > 0
        ),
        'reviews', jsonb_build_object(
            'count', COALESCE(r.cnt, 0),
            'avg_rating', COALESCE(ROUND(r.avg_rating, 2), 0),
            'verified_count', COALESCE(r.verified, 0)
        )
    ) AS data
FROM tb_product p
JOIN tb_category c ON c.pk_category = p.fk_category
LEFT JOIN tb_supplier s ON s.pk_supplier = p.fk_supplier
LEFT JOIN tb_inventory i ON i.fk_product = p.pk_product
LEFT JOIN (
    SELECT fk_product,
           count(*) AS cnt,
           avg(rating) AS avg_rating,
           count(*) FILTER (WHERE verified_purchase) AS verified
    FROM tb_review
    GROUP BY fk_product
) r ON r.fk_product = p.pk_product
