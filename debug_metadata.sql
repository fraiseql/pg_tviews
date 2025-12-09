-- Debug script: Check what's in pg_tview_meta after TVIEW creation
\c test_cascade

-- Show all TVIEWs and their dependencies
SELECT
    entity,
    view_oid,
    table_oid,
    array_length(dependencies, 1) AS dep_count,
    dependencies
FROM pg_tview_meta;

-- Show all base tables with their OIDs
SELECT
    oid,
    relname
FROM pg_class
WHERE relkind = 'r'
  AND relname IN ('tb_user', 'tb_post');
