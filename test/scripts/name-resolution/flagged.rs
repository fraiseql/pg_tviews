// Every string literal below resolves a name to an object: each must be flagged by
// scripts/check-name-resolution.sh (test/scripts/check-name-resolution-test.sh).
fn flagged() {
    let _ = "SELECT pg_catalog.to_regclass($1)";
    let _ = "SELECT $1::regclass";
    let _ = "SELECT CAST($1 AS regclass)";
    let _ = "SELECT CAST(x AS pg_catalog.regclass) FROM t";
    let _ = "SELECT $1::pg_catalog.\"regclass\"";
    let _ = "SELECT ($1)::regclass";
    let _ = "SELECT $1::text[]::regclass[]";
    let _ = "SELECT format('%I.%I', $1, $2)::regclass";
    let _ = format!("SELECT '{name:?}'::regclass");
    let _ = format!("SELECT {name:?}::regclass");
    let _ = "SELECT $1::text\
             ::pg_catalog.regclass";
    let _ = "SELECT pg_catalog.regclass($1)";
    let _ = "SELECT pg_catalog.pg_relation_size('public.t')";
    let _ = format!("SELECT pg_catalog.pg_total_relation_size('{}')", name);
    let _ = "SELECT pg_catalog.has_table_privilege(current_user, 'public.t', 'SELECT')";
    let _ = "SELECT nextval('public.s')";
    let _ = r#"SELECT 'x.y'::regtype"#;
    let _ = "SELECT $1::regprocedure";
}
