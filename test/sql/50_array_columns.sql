-- Array Column Materialization Tests

-- This test verifies that TVIEWs can materialize array columns correctly

\set ON_ERROR_STOP on

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

    -- Test Case 1: Array column materialization with UUID arrays
    CREATE TABLE tb_machine (
        pk_machine INTEGER PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid(),
        serial_number TEXT
    );

    CREATE TABLE tb_machine_item (
        pk_machine_item INTEGER PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid(),
        fk_machine INTEGER REFERENCES tb_machine(pk_machine),
        name TEXT
    );

    INSERT INTO tb_machine VALUES (1, gen_random_uuid(), 'M-001');
    INSERT INTO tb_machine_item VALUES (1, gen_random_uuid(), 1, 'Item A');
    INSERT INTO tb_machine_item VALUES (2, gen_random_uuid(), 1, 'Item B');

    -- Create TVIEW with array column
    SELECT pg_tviews_create('machine', $$
        SELECT
            m.pk_machine,
            m.id,
            m.serial_number,
            ARRAY(
                SELECT mi.id
                FROM tb_machine_item mi
                WHERE mi.fk_machine = m.pk_machine
                ORDER BY mi.pk_machine_item
            ) AS machine_item_ids,
            jsonb_build_object(
                'id', m.id,
                'serial_number', m.serial_number,
                'items', (
                    SELECT jsonb_agg(jsonb_build_object('id', mi.id, 'name', mi.name) ORDER BY mi.pk_machine_item)
                    FROM tb_machine_item mi
                    WHERE mi.fk_machine = m.pk_machine
                )
            ) AS data
        FROM tb_machine m
    $$);
    SELECT assert_fresh('tv_machine', 'pk_machine', 'pg_tviews_create');

    -- Test 1: Array column exists with correct type
    SELECT
        column_name,
        data_type,
        is_nullable
    FROM information_schema.columns
    WHERE table_name = 'tv_machine'
      AND column_name = 'machine_item_ids';

    -- Expected: machine_item_ids | ARRAY | YES. The column keeps the element
    -- type of the ARRAY(...) subquery; nullability is not inferred from it.
    DO $$ BEGIN
        IF (SELECT format_type(atttypid, atttypmod) FROM pg_attribute
            WHERE attrelid = 'tv_machine'::regclass AND attname = 'machine_item_ids'
              AND NOT attisdropped) IS DISTINCT FROM 'uuid[]' THEN
            RAISE EXCEPTION 'FAIL: tv_machine.machine_item_ids is not uuid[]';
        END IF;
    END $$;

    -- Test 2: Array populated correctly (if column exists)
    SELECT
        pk_machine,
        array_length(machine_item_ids, 1) AS array_length,
        machine_item_ids[1] IS NOT NULL AS has_first_element,
        machine_item_ids[2] IS NOT NULL AS has_second_element
    FROM tv_machine
    WHERE pk_machine = 1;

    -- Expected: 1 | 2 | t | t
    DO $$ BEGIN
        IF (SELECT machine_item_ids FROM tv_machine WHERE pk_machine = 1)
           IS DISTINCT FROM (SELECT array_agg(id ORDER BY pk_machine_item)
                             FROM tb_machine_item WHERE fk_machine = 1) THEN
            RAISE EXCEPTION 'FAIL: tv_machine.machine_item_ids is not the two item ids in order';
        END IF;
    END $$;

    -- Test 3: JSONB array in data column works
    SELECT
        jsonb_array_length(data->'items') AS jsonb_array_length,
        data->'items'->0->>'name' AS first_item_name,
        data->'items'->1->>'name' AS second_item_name
    FROM tv_machine
    WHERE pk_machine = 1;

    -- Expected: 2 | Item A | Item B
    DO $$ BEGIN
        IF (SELECT jsonb_path_query_array(data, '$.items[*].name') FROM tv_machine
            WHERE pk_machine = 1) IS DISTINCT FROM '["Item A", "Item B"]'::jsonb THEN
            RAISE EXCEPTION 'FAIL: tv_machine data.items is not [Item A, Item B]';
        END IF;
    END $$;

ROLLBACK;