-- This test verifies that batch updates work efficiently for large cascades

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

    -- Test Case 1: Batch update optimization for large cascades
    CREATE TABLE tb_company (
        pk_company INTEGER PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid(),
        name TEXT
    );

    CREATE TABLE tb_user (
        pk_user INTEGER PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid(),
        fk_company INTEGER REFERENCES tb_company(pk_company),
        name TEXT
    );

    INSERT INTO tb_company VALUES (1, gen_random_uuid(), 'TechCorp');

    -- Insert 50 users for this company (medium cascade test)
    INSERT INTO tb_user
    SELECT i, gen_random_uuid(), 1, 'User ' || i
    FROM generate_series(1, 50) i;

    -- Create TVIEW with company data including user array
    SELECT pg_tviews_create('company', $$
        SELECT
            c.pk_company,
            c.id,
            c.name,
            jsonb_build_object(
                'id', c.id,
                'name', c.name,
                'users', COALESCE(
                    jsonb_agg(
                        jsonb_build_object('id', u.id, 'name', u.name)
                        ORDER BY u.pk_user
                    ) FILTER (WHERE u.pk_user IS NOT NULL),
                    '[]'::jsonb
                )
            ) AS data
        FROM tb_company c
        LEFT JOIN tb_user u ON u.fk_company = c.pk_company
        GROUP BY c.pk_company, c.id, c.name
    $$);

    -- Verify initial state
    SELECT
        jsonb_array_length(data->'users') AS initial_user_count
    FROM tv_company
    WHERE pk_company = 1;
    -- Expected: 50

    -- Benchmark: Update company name (affects all 50 users)
    -- This should use batch optimization (>10 rows affected)
    \timing on
    UPDATE tb_company SET name = 'TechCorp Updated' WHERE pk_company = 1;
    \timing off

    -- Verify all users updated (company name should appear in each user context if needed)
    -- For this test, we verify the update completed and data is consistent
    SELECT
        jsonb_array_length(data->'users') AS after_update_user_count,
        data->>'name' AS company_name
    FROM tv_company
    WHERE pk_company = 1;
    -- Expected: 50 | TechCorp Updated

    -- Test 2: Small cascade (should use individual updates, not batch)
    -- Create another company with only 3 users
    INSERT INTO tb_company VALUES (2, gen_random_uuid(), 'SmallCo');
    INSERT INTO tb_user VALUES (51, gen_random_uuid(), 2, 'Small User 1');
    INSERT INTO tb_user VALUES (52, gen_random_uuid(), 2, 'Small User 2');
    INSERT INTO tb_user VALUES (53, gen_random_uuid(), 2, 'Small User 3');

    -- Update small company (should use individual updates)
    \timing on
    UPDATE tb_company SET name = 'SmallCo Updated' WHERE pk_company = 2;
    \timing off

    -- Verify small company updated correctly
    SELECT
        jsonb_array_length(data->'users') AS small_company_user_count,
        data->>'name' AS small_company_name
    FROM tv_company
    WHERE pk_company = 2;
    -- Expected: 3 | SmallCo Updated

ROLLBACK;