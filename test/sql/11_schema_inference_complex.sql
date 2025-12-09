-- test/sql/11_schema_inference_complex.sql
-- Test: Complex schema with FKs, UUID FKs, arrays, flags

BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Test: Complex schema with FKs, UUID FKs, arrays, flags
    SELECT jsonb_pretty(
        pg_tviews_analyze_select($$
            SELECT
                a.pk_allocation,
                a.id,
                a.fk_machine,
                a.fk_location,
                m.id AS machine_id,
                l.id AS location_id,
                a.tenant_id,
                (a.start_date <= CURRENT_DATE) AS is_current,
                (a.end_date < CURRENT_DATE) AS is_past,
                ARRAY(SELECT mi.id FROM tb_machine_item mi) AS machine_item_ids,
                jsonb_build_object('id', a.id) AS data
            FROM tb_allocation a
        $$)
    );

ROLLBACK;