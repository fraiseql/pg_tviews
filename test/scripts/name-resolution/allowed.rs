// Nothing below resolves a name: casts of OIDs, pg_catalog literals, Rust paths
// and comments. None may be flagged.
fn allowed() {
    let _ = "SELECT c.oid::pg_catalog.regclass FROM pg_catalog.pg_class c";
    let _ = "SELECT $1::pg_catalog.oid::pg_catalog.regclass";
    let _ = "SELECT $3::pg_catalog.oid[]::pg_catalog.regclass[]";
    let _ = "SELECT 'pg_catalog.pg_class'::pg_catalog.regclass";
    let _ = "SELECT 'pg_catalog'::pg_catalog.regnamespace";
    let _ = "SELECT CAST(c.oid AS pg_catalog.regclass) FROM pg_catalog.pg_class c";
    let _ = "SELECT pg_catalog.pg_relation_size(c.oid) FROM pg_catalog.pg_class c";
    let _ = "SELECT pg_catalog.has_table_privilege($1::pg_catalog.oid, 'SELECT')";
    let _ = "SELECT o.tgrelid::pg_catalog.regclass FROM pg_catalog.pg_trigger o";
    let _ = "SELECT d.refobjid::pg_catalog.regclass FROM pg_catalog.pg_depend d";
    let _ = "SELECT c.oid::\"pg_catalog\".regclass FROM pg_catalog.pg_class c";
    let _ = "SELECT pg_catalog.pg_partition_root($1::pg_catalog.oid)";
    let _ = "SELECT pg_catalog.pg_get_viewdef(c.oid) FROM pg_catalog.pg_class c";
    let _ = format!("{} has no rows", mapping.table);
    let _ = format!("SELECT 1 FROM {r:relid} s");
    let _ = crate::utils::spi::regclass(oid);
    // SELECT $1::regclass in a comment does not count.
    let _ = "the regclass of a table, in prose";
}
