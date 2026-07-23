use crate::cascade_path;
use crate::error::{TViewError, TViewResult};
use crate::schema::{
    TViewSchema, analyzer::analyze_dependencies, direct_map::extract_direct_column_map,
    inference::infer_schema,
};
use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;

/// Resolve the target schema for creating TVIEW objects.
///
/// Uses `current_schema()` to respect the active `search_path`, matching
/// standard `PostgreSQL` convention for unqualified DDL statements.
fn current_schema() -> TViewResult<String> {
    crate::utils::spi_get_string("SELECT current_schema()::text")
        .map_err(|e| TViewError::CatalogError {
            operation: "Get current schema".to_string(),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| TViewError::CatalogError {
            operation: "Get current schema".to_string(),
            pg_error: "current_schema() returned NULL (no schema in search_path?)".to_string(),
        })
}

/// Expand `SELECT * FROM [schema.]source` to an explicit column list.
///
/// When the SELECT is just `SELECT * FROM …`, `pg_tviews` cannot infer the
/// Trinity schema (pk_*, id, data columns) from the wildcard at parse time.
/// This function detects that pattern and expands it by querying
/// `information_schema.columns` for the source view/table's actual columns,
/// preserving their declaration order.
///
/// Returns the original SQL unchanged if it is not a simple `SELECT *`.
///
/// # Errors
/// Returns error only if the `information_schema` query itself fails.
/// A missing source or empty column list silently returns the original SQL
/// so the caller can fall through to the normal error path.
fn expand_select_star_if_needed(select_sql: &str) -> TViewResult<String> {
    let trimmed = select_sql.trim();
    let lower = trimmed.to_lowercase();

    // Must start with SELECT
    let after_kw = lower.strip_prefix("select").unwrap_or("").trim_start();

    // Must have * immediately after SELECT (not SELECT DISTINCT * or SELECT t.*)
    let after_star = match after_kw.strip_prefix('*') {
        Some(rest) => rest.trim_start(),
        None => return Ok(select_sql.to_string()),
    };

    // The token after * must be FROM (no other clauses like WHERE before FROM)
    let after_from = match after_star.strip_prefix("from") {
        Some(rest) if rest.chars().next().is_none_or(|c| c.is_ascii_whitespace()) => {
            rest.trim_start()
        }
        _ => return Ok(select_sql.to_string()),
    };

    // Extract the source name: everything after FROM up to whitespace/semicolon
    let source_qualified = after_from
        .trim_end_matches(';')
        .trim()
        .split_ascii_whitespace()
        .next()
        .unwrap_or("");

    if source_qualified.is_empty() {
        return Ok(select_sql.to_string());
    }

    // Parse optional schema qualifier: "schema.table" or just "table"
    let (schema_name, table_name) = match source_qualified.split_once('.') {
        Some((s, t)) => (
            Some(s.trim_matches('"').to_string()),
            t.trim_matches('"').to_string(),
        ),
        None => (None, source_qualified.trim_matches('"').to_string()),
    };

    // Query information_schema.columns for the column names in order
    let columns: Vec<String> = if let Some(ref schema) = schema_name {
        // SAFETY: DatumWithOid::new wraps PostgreSQL datum pointers for SPI parameter passing.
        // The OID parameter ensures correct type handling in PostgreSQL. Validated strings
        // from table/schema names are passed as text OID parameters.
        let args = vec![
            unsafe {
                pgrx::datum::DatumWithOid::new(
                    table_name.as_str(),
                    pgrx::prelude::PgOid::BuiltIn(pgrx::prelude::PgBuiltInOids::TEXTOID).value(),
                )
            },
            unsafe {
                pgrx::datum::DatumWithOid::new(
                    schema.as_str(),
                    pgrx::prelude::PgOid::BuiltIn(pgrx::prelude::PgBuiltInOids::TEXTOID).value(),
                )
            },
        ];
        pgrx::prelude::Spi::connect(|client| {
            let rows = client.select(
                "SELECT column_name::text \
                 FROM information_schema.columns \
                 WHERE table_name = $1 AND table_schema = $2 \
                 ORDER BY ordinal_position",
                None,
                &args,
            )?;
            let mut result = Vec::new();
            for row in rows {
                if let Some(col) =
                    row[1]
                        .value::<String>()
                        .map_err(|e| TViewError::CatalogError {
                            operation: "expand_select_star: read column_name".to_string(),
                            pg_error: format!("{e:?}"),
                        })?
                {
                    result.push(col);
                }
            }
            Ok(result)
        })
        .map_err(|e: pgrx::spi::Error| TViewError::SpiError {
            query: "expand_select_star: information_schema query".to_string(),
            error: e.to_string(),
        })?
    } else {
        let args = vec![unsafe {
            pgrx::datum::DatumWithOid::new(
                table_name.as_str(),
                pgrx::prelude::PgOid::BuiltIn(pgrx::prelude::PgBuiltInOids::TEXTOID).value(),
            )
        }];
        pgrx::prelude::Spi::connect(|client| {
            let rows = client.select(
                "SELECT column_name::text \
                 FROM information_schema.columns \
                 WHERE table_name = $1 \
                 ORDER BY ordinal_position",
                None,
                &args,
            )?;
            let mut result = Vec::new();
            for row in rows {
                if let Some(col) =
                    row[1]
                        .value::<String>()
                        .map_err(|e| TViewError::CatalogError {
                            operation: "expand_select_star: read column_name".to_string(),
                            pg_error: format!("{e:?}"),
                        })?
                {
                    result.push(col);
                }
            }
            Ok(result)
        })
        .map_err(|e: pgrx::spi::Error| TViewError::SpiError {
            query: "expand_select_star: information_schema query".to_string(),
            error: e.to_string(),
        })?
    };

    if columns.is_empty() {
        // Source not found or no columns — fall through to normal path
        return Ok(select_sql.to_string());
    }

    // Build explicit SELECT preserving original source reference (with schema prefix)
    let col_list = columns.join(", ");
    Ok(format!("SELECT {col_list} FROM {source_qualified}"))
}

/// Create a TVIEW with atomic rollback on error
///
/// This is the main entry point for CREATE TABLE tv_ AS SELECT .... `PostgreSQL`'s transaction
/// system automatically provides atomicity - if any step fails, all changes
/// are rolled back.
///
/// `schema_override` is the explicit schema name extracted from the DDL statement
/// (e.g. `"public"` for `CREATE TABLE public.tv_org AS SELECT …`).  When `None`,
/// the target schema is resolved from `current_schema()` at call time.  Callers
/// that intercept a schema-qualified DDL statement MUST pass the schema here so
/// that the TVIEW lands in the correct schema even when the database's
/// `search_path` would resolve `current_schema()` to a different schema.
///
/// Steps:
/// 1. Check if TVIEW already exists
/// 2. Expand SELECT * if needed, then infer schema from SELECT statement
/// 3. Create backing view v_<entity>
/// 4. Create materialized table tv_<entity>
/// 5. Populate initial data
/// 6. Register metadata
/// 7. Find base table dependencies and install triggers
///
/// # Errors
/// Returns error if TVIEW already exists, SQL is invalid, or trigger installation fails
pub fn create_tview(
    tview_name: &str,
    select_sql: &str,
    schema_override: Option<&str>,
    defer_populate: bool,
) -> TViewResult<()> {
    // Step 1: Check if TVIEW already exists
    let exists = tview_exists(tview_name)?;
    if exists {
        return Err(TViewError::TViewAlreadyExists {
            name: tview_name.to_string(),
        });
    }

    // Step 1.5: Extract entity name from tview_name
    // Support both "tv_entity" and just "entity" formats
    let entity_name = tview_name
        .strip_prefix("tv_")
        .map_or(tview_name, |stripped| stripped);

    // Step 1.6: Expand SELECT * → explicit column list so infer_schema can
    // recognise the Trinity Pattern (pk_*, id, data) even when the DDL uses
    // `CREATE TABLE tv_foo AS SELECT * FROM v_foo_base`.
    let select_sql = expand_select_star_if_needed(select_sql)?;
    let select_sql = select_sql.as_str();

    // Step 2: Infer schema from SELECT
    // If SELECT doesn't have TVIEW format (pk_<entity>, id, data), create a prepared view first
    let schema = infer_schema(select_sql)?;

    // Check if we need to transform the SELECT to TVIEW format
    let (final_select_sql, final_schema) = if schema.entity_name.is_none() {
        // Raw SELECT - needs transformation to TVIEW format
        transform_raw_select_to_tview(entity_name, select_sql)?
    } else {
        // Already in TVIEW format
        (select_sql.to_string(), schema)
    };

    let entity_name =
        final_schema
            .entity_name
            .as_ref()
            .ok_or_else(|| TViewError::RequiredColumnMissing {
                column_name: format!(
                    "pk_{}",
                    tview_name.strip_prefix("tv_").unwrap_or(tview_name)
                ),
                context: "pg_tviews requires a Trinity Pattern primary key column named \
                      \"pk_<entity>\" (e.g., pk_user, pk_post)"
                    .to_string(),
            })?;

    // Validate entity_name inferred from the SELECT to prevent SQL injection
    // (tview_name is validated at the pg_extern boundary, but entity_name comes
    // from infer_schema and could contain metacharacters if the user crafts a
    // malicious column alias like pk_evil'injection).
    crate::validation::validate_sql_identifier(entity_name, "entity_name")?;

    // Derive the canonical materialized-table name: always tv_<entity>.
    // This normalises both calling conventions:
    //   pg_tviews_create('post', ...)   → tv_post
    //   pg_tviews_create('tv_post', ...) → tv_post
    let tv_table_name = format!("tv_{entity_name}");

    // Resolve the target schema.  Prefer the caller-supplied override (extracted from
    // the DDL statement) so that `CREATE TABLE public.tv_foo AS SELECT …` always
    // creates in "public" even when the database default search_path resolves
    // `current_schema()` to a different schema (e.g. "app").
    let schema_name = match schema_override {
        Some(s) => s.to_string(),
        None => current_schema()?,
    };

    // Reject WITH RECURSIVE up front (issue #51): cascade paths cannot be tracked
    // through a recursive CTE, so creating one would leave a tview that silently
    // refreshes incompletely.
    if crate::sql_parser::has_recursive_cte(&final_select_sql) {
        return Err(TViewError::InvalidInput {
            parameter: "tview definition".to_string(),
            reason: format!(
                "TVIEW '{tv_table_name}' uses WITH RECURSIVE, which pg_tviews does not support: \
                 cascade paths cannot be tracked through a recursive CTE, so the tview would \
                 refresh incompletely. Rewrite the definition without recursion."
            ),
        });
    }

    // Resolve DISTINCT ON keys before creating any objects, so an unresolvable
    // dedup key rejects the create cleanly (issue #51). `distinct_on_keys` is the
    // raw SOURCE column the trigger reads off the base tuple; `distinct_on_output_keys`
    // is the projected OUTPUT column the view / tv expose and that refresh + the
    // tview primary key must use. They differ when the DISTINCT ON key is aliased
    // (e.g. `c.id_contract AS pk_contract`).
    let distinct_on_keys =
        crate::schema::parser::extract_distinct_on_keys(&final_select_sql).unwrap_or_default();
    let distinct_on_output_keys = if distinct_on_keys.is_empty() {
        Vec::new()
    } else {
        crate::sql_parser::extract_distinct_on_output_keys(&final_select_sql).map_err(|reason| {
            TViewError::InvalidInput {
                parameter: "DISTINCT ON key".to_string(),
                reason,
            }
        })?
    };

    // Step 3: Create backing view v_<entity>
    let view_name = format!("v_{entity_name}");
    create_backing_view(&view_name, &final_select_sql, &schema_name)?;

    // Step 4: Create materialized table tv_<entity>.
    // DISTINCT ON tviews key the table on the projected OUTPUT column.
    create_materialized_table(
        &tv_table_name,
        &final_schema,
        &schema_name,
        &distinct_on_output_keys,
    )?;

    // Step 5: Populate initial data
    // When called from the ddl_command_end event trigger (CTAS path), the DDL
    // sub-transactions corrupt savepoint depth tracking and the INSERT's effects
    // are lost.  Defer the populate to after the event trigger returns, where
    // the ProcessUtility hook drains the queue in a clean SPI context.
    if defer_populate {
        crate::hooks::enqueue_pending_populate(&tv_table_name, &view_name, &schema_name);
    } else {
        populate_initial_data(&tv_table_name, &view_name, &final_schema, &schema_name)?;
    }

    // Step 6: Find base table dependencies.
    // Pass schema_name so the view OID lookup searches in the correct schema even when
    // current_schema() resolves to a different schema due to the database search_path.
    let dep_graph = crate::dependency::find_base_tables(&view_name, Some(&schema_name))?;

    // Step 6.5: Extract cascade paths from the SELECT SQL
    let cascade_paths = extract_and_resolve_cascade_paths(
        &final_select_sql,
        entity_name,
        &final_schema,
        &dep_graph.base_tables,
        &schema_name,
    )?;

    // Step 6.55: Reject a DISTINCT ON tview that also has JOIN-based cascade paths
    // (issue #51). A DISTINCT ON tview refreshes by dedup key (the trigger enqueues
    // the DISTINCT ON key value); a cascade path enqueues a base-table PK that
    // `refresh_pk` looks up by `pk_<entity>`. When the dedup key is aliased the two
    // are different values, so mixing them would refresh the wrong row silently.
    // The dedup and PK refresh paths are mutually exclusive by design.
    if !distinct_on_keys.is_empty() && !cascade_paths.is_empty() {
        return Err(TViewError::InvalidInput {
            parameter: "tview definition".to_string(),
            reason: format!(
                "TVIEW '{tv_table_name}' combines DISTINCT ON with a JOIN that produces \
                 cascade paths ({} dependent table(s)). DISTINCT ON tviews refresh by \
                 deduplication key and cannot also track PK-based cascade dependencies — the \
                 two refresh paths are mutually exclusive. Remove the DISTINCT ON, or express \
                 the dependency without a join that pg_tviews would cascade.",
                cascade_paths.len()
            ),
        });
    }

    // Step 6.6: Reject a tview that can never be incrementally refreshed (issue #49).
    // A refresh is enqueued for this entity only if either a `tb_<entity>` base table
    // exists (its row trigger resolves the entity by stripping `tb_`) or a cascade
    // path routes some base-table change to it. With neither, the tview would silently
    // never refresh — and, when built directly on a `tb_` base table that already
    // backs another entity (e.g. `order_summary` over `tb_order`), shadow that
    // sibling. Aggregate/summary tviews without a `tb_<entity>` stay valid as long as
    // their joins produce cascade paths. This runs before metadata registration and
    // trigger installation, and any objects created above roll back with the ERROR,
    // so a rejected create leaves the incumbent tview untouched.
    if cascade_paths.is_empty() && !entity_base_table_exists(entity_name, &schema_name)? {
        return Err(TViewError::InvalidInput {
            parameter: "tview definition".to_string(),
            reason: format!(
                "TVIEW '{tv_table_name}' (entity '{entity_name}') can never be refreshed: \
                 there is no base table 'tb_{entity_name}', and no cascade path routes any \
                 base-table change to it. pg_tviews maintains a tview either directly (a \
                 pk_<entity> primary key over a tb_<entity> base table) or via cascade \
                 paths from its joined base tables. Rename the primary-key column to match \
                 an existing base table, or ensure the definition joins the base tables it \
                 derives from. Creating it as-is would leave a permanently stale tview and \
                 can silently shadow a correctly-named tview on the same base table."
            ),
        });
    }

    // Step 7: Register metadata (with cascade paths)
    register_metadata(
        entity_name,
        &view_name,
        &tv_table_name,
        &final_select_sql,
        &final_schema,
        &cascade_paths,
        &schema_name,
        &distinct_on_keys,
        &distinct_on_output_keys,
    )?;

    // Step 8: Install triggers on base tables
    if dep_graph.base_tables.is_empty() {
        warning!("No base table dependencies found for {}", tv_table_name);
    } else {
        crate::dependency::install_triggers(&dep_graph.base_tables, entity_name)?;
    }

    // Invalidate caches since new TVIEW was created
    crate::queue::cache::invalidate_all_caches();

    // Buffer and flush audit entry immediately (we're in SPI context)
    crate::audit::log_create(entity_name, select_sql);
    if let Err(e) = crate::audit::flush_audit_buffer() {
        warning!("Failed to flush audit after CREATE: {}", e);
    }

    Ok(())
}

/// Extract cascade paths from the view's SELECT SQL and resolve table names to OIDs.
///
/// Parses the JOIN tree to find all non-root tables, computes the hop chain
/// from each back to the root (= the TVIEW's own base table), then resolves
/// table names to OIDs and validates that all referenced columns exist.
fn extract_and_resolve_cascade_paths(
    select_sql: &str,
    entity_name: &str,
    _schema: &TViewSchema,
    base_table_oids: &[pg_sys::Oid],
    schema_name: &str,
) -> TViewResult<Vec<cascade_path::CascadePath>> {
    let root_table = format!("tb_{entity_name}");

    // Step 1: Parse JOIN tree to extract unresolved paths
    let join_paths = match crate::sql_parser::extract_join_paths(select_sql, &root_table) {
        Ok(paths) => paths,
        Err(e) => {
            notice!("Could not parse JOIN tree for cascade paths: {e}");
            return Ok(vec![]);
        }
    };

    if join_paths.is_empty() {
        return Ok(vec![]);
    }

    // Step 2: Build OID lookup map for base tables: relname → OID
    let oid_map = build_oid_name_map(base_table_oids)?;

    // Step 3: Resolve each join path to a CascadePath with OIDs
    let mut cascade_paths = Vec::new();
    for jp in &join_paths {
        match resolve_join_path(jp, entity_name, &oid_map) {
            Ok(mut cp) => {
                // Column-aware refresh: record which columns of this path's source
                // table the target tview actually depends on (via pg_depend on the
                // backing view). Empty ⇒ always refresh.
                cp.source_columns =
                    view_source_columns(schema_name, entity_name, &jp.source_table);
                cascade_paths.push(cp);
            }
            Err(e) => {
                notice!(
                    "Cascade path from '{}' unresolvable: {e} — changes to this table will not trigger incremental refresh of '{entity_name}'",
                    jp.source_table
                );
            }
        }
    }

    Ok(cascade_paths)
}

/// Build a map from table name → OID for a set of base table OIDs.
fn build_oid_name_map(
    oids: &[pg_sys::Oid],
) -> TViewResult<std::collections::HashMap<String, pg_sys::Oid>> {
    use std::collections::HashMap;

    if oids.is_empty() {
        return Ok(HashMap::new());
    }

    let oid_list = oids
        .iter()
        .map(|o| o.to_u32().to_string())
        .collect::<Vec<_>>()
        .join(",");

    let query = format!("SELECT oid, relname::text FROM pg_class WHERE oid IN ({oid_list})");

    let mut map = HashMap::new();
    Spi::connect(|client| {
        let rows = client.select(&query, None, &[])?;
        for row in rows {
            let oid: pg_sys::Oid = row["oid"].value()?.unwrap_or(pg_sys::Oid::INVALID);
            let name: String = row["relname"].value()?.unwrap_or_default();
            map.insert(name, oid);
        }
        Ok::<_, spi::Error>(())
    })?;

    Ok(map)
}

/// Resolve a `JoinPath` (table names only) to a `CascadePath` (with OIDs).
fn resolve_join_path(
    jp: &crate::sql_parser::JoinPath,
    entity_name: &str,
    oid_map: &std::collections::HashMap<String, pg_sys::Oid>,
) -> TViewResult<cascade_path::CascadePath> {
    let source_oid = *oid_map
        .get(&jp.source_table)
        .ok_or_else(|| TViewError::CatalogError {
            operation: "resolve cascade path".to_string(),
            pg_error: format!(
                "Table '{}' not found in base table dependencies",
                jp.source_table
            ),
        })?;

    let mut hops = Vec::with_capacity(jp.steps.len());
    for step in &jp.steps {
        let table_oid = *oid_map
            .get(&step.table_name)
            .ok_or_else(|| TViewError::CatalogError {
                operation: "resolve cascade path hop".to_string(),
                pg_error: format!(
                    "Intermediate table '{}' not found in base table dependencies",
                    step.table_name
                ),
            })?;

        hops.push(cascade_path::CascadeHop {
            table_oid,
            table_name: step.table_name.clone(),
            lookup_col: step.lookup_col.clone(),
            carry_col: step.carry_col.clone(),
        });
    }

    // Check if the path terminates at the entity PK. If root_join_col is
    // NOT pk_{entity}, the incoming IDs are foreign keys on the root table
    // (not entity PKs), so we need a reverse-lookup hop through the root.
    let pk_col = format!("pk_{entity_name}");
    if jp.root_join_col != pk_col {
        let root_table = format!("tb_{entity_name}");
        let root_oid = *oid_map
            .get(&root_table)
            .ok_or_else(|| TViewError::CatalogError {
                operation: "resolve cascade path reverse-lookup hop".to_string(),
                pg_error: format!("Root table '{root_table}' not found in base table dependencies"),
            })?;
        hops.push(cascade_path::CascadeHop {
            table_oid: root_oid,
            table_name: root_table,
            lookup_col: jp.root_join_col.clone(),
            carry_col: pk_col,
        });
    }

    Ok(cascade_path::CascadePath {
        source_oid,
        source_table: jp.source_table.clone(),
        entity_name: entity_name.to_string(),
        initial_col: jp.initial_col.clone(),
        hops,
        unresolvable: false,
        // Populated by the caller (which has the schema) via `view_source_columns`.
        source_columns: Vec::new(),
    })
}

/// Columns of `source_table` that the backing view `v_<entity>` depends on,
/// read from `PostgreSQL`'s own column-level `pg_depend` records. This is the
/// exact set of source columns whose change can alter a target tview row —
/// it correctly accounts for expressions, `SELECT *` expansion, and repeated
/// joins, with none of the fragility of parsing the SELECT text.
///
/// Returns an empty vec on any error or when the view has no *direct* column
/// dependency on `source_table` (e.g. a multi-hop cascade whose backing view
/// references an intermediate view, not the leaf table). The caller treats an
/// empty result as "unknown ⇒ always refresh", so a miss is never unsafe.
fn view_source_columns(schema: &str, entity: &str, source_table: &str) -> Vec<String> {
    // All three identifiers are bound as text parameters (no in-band SQL quoting)
    // and cast to regnamespace/regclass by PostgreSQL.
    const QUERY: &str = "SELECT a.attname::text AS col \
         FROM pg_depend d \
         JOIN pg_rewrite r ON r.oid = d.objid \
         JOIN pg_class v ON v.oid = r.ev_class \
         JOIN pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
         WHERE v.relname = $1 AND v.relnamespace = $2::regnamespace \
           AND d.refobjid = $3::regclass AND d.refobjsubid > 0";
    let view_name = format!("v_{entity}");
    // Schema-qualified, identifier-quoted name for the ::regclass lookup.
    let qi_src = format!(
        "{}.{}",
        quote_identifier(schema),
        quote_identifier(source_table)
    );
    let mut cols = Vec::new();
    let result = Spi::connect(|client| {
        // SAFETY: DatumWithOid::new wraps datum pointers for SPI parameter passing;
        // view_name/schema/qi_src outlive this select call.
        let text = PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value();
        let args = vec![
            unsafe { DatumWithOid::new(view_name.as_str(), text) },
            unsafe { DatumWithOid::new(schema, text) },
            unsafe { DatumWithOid::new(qi_src.as_str(), text) },
        ];
        let rows = client.select(QUERY, None, &args)?;
        for row in rows {
            if let Ok(Some(name)) = row["col"].value::<String>() {
                cols.push(name);
            }
        }
        Ok::<_, spi::Error>(())
    });
    if let Err(e) = result {
        notice!(
            "view_source_columns({view_name}, {source_table}): {e} — cascade will always refresh"
        );
        return Vec::new();
    }
    cols
}

/// Check if a TVIEW already exists
fn tview_exists(tview_name: &str) -> TViewResult<bool> {
    let entity_name = tview_name.trim_start_matches("tv_");
    let args = vec![unsafe {
        DatumWithOid::new(entity_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
    }];

    Spi::get_one_with_args::<bool>(
        "SELECT COUNT(*) > 0 FROM pg_tview_meta WHERE entity = $1",
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check TVIEW exists: {tview_name}"),
        pg_error: format!("{e:?}"),
    })
    .map(|opt| opt.unwrap_or(false))
}

/// Does a `tb_<entity>` base table exist for this entity?
///
/// Used by the issue #49 refreshability guard: a `tb_<entity>` base table means the
/// entity is reachable directly (its row trigger resolves the entity by stripping
/// `tb_`). The table is resolved in the tview's target schema first, then via the
/// active `search_path`, so both explicit-schema and `current_schema()` callers are
/// handled.
fn entity_base_table_exists(entity_name: &str, schema_name: &str) -> TViewResult<bool> {
    let tb_name = format!("tb_{entity_name}");
    let qualified = format!(
        "{}.{}",
        quote_identifier(schema_name),
        quote_identifier(&tb_name)
    );

    Spi::get_one_with_args::<bool>(
        "SELECT COALESCE(to_regclass($1), to_regclass($2)) IS NOT NULL",
        &[
            unsafe {
                DatumWithOid::new(
                    qualified.as_str(),
                    PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
                )
            },
            unsafe {
                DatumWithOid::new(
                    tb_name.as_str(),
                    PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
                )
            },
        ],
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check base table exists for entity '{entity_name}'"),
        pg_error: format!("{e:?}"),
    })
    .map(|opt| opt.unwrap_or(false))
}

/// Create the backing view that contains the user's SELECT definition
fn create_backing_view(view_name: &str, select_sql: &str, schema_name: &str) -> TViewResult<()> {
    let qi_schema = quote_identifier(schema_name);
    let qi_view = quote_identifier(view_name);
    let create_view_sql = format!("CREATE VIEW {qi_schema}.{qi_view} AS {select_sql}");

    // Use notice! instead of info! to ensure visibility
    notice!(
        "DEBUG: create_backing_view START - schema='{}', view='{}', sql_len={}",
        schema_name,
        view_name,
        create_view_sql.len()
    );

    match crate::utils::spi_run_ddl(&create_view_sql) {
        Ok(()) => {
            // Log successful spi_run_ddl
            notice!(
                "DEBUG: spi_run_ddl SUCCEEDED for {}.{}",
                schema_name,
                view_name
            );
        }
        Err(e) => {
            // Log spi_run_ddl failure
            notice!(
                "DEBUG: spi_run_ddl FAILED - {}.{} - error: {}",
                schema_name,
                view_name,
                e
            );
            // Note: error!() macro diverges, so this return is unreachable but needed for type checking
            return Err(TViewError::SpiError {
                query: create_view_sql.clone(),
                error: e,
            });
        }
    }

    // Log before verification check
    notice!(
        "DEBUG: checking if view exists - schema='{}', view='{}' in pg_class",
        schema_name,
        view_name
    );

    // Verify the view was created (schema-qualified to avoid false positives across schemas)
    let check_args = vec![
        unsafe { DatumWithOid::new(view_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe { DatumWithOid::new(schema_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
    ];
    let exists = match Spi::get_one_with_args::<i32>(
        "SELECT 1 FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = $1 AND n.nspname = $2 AND c.relkind = 'v'",
        &check_args,
    ) {
        Ok(result) => {
            if result.is_some() {
                // Log successful verification
                notice!(
                    "DEBUG: VERIFIED - backing view {}.{} exists in pg_class",
                    schema_name,
                    view_name
                );
                true
            } else {
                // Log verification failure
                notice!(
                    "DEBUG: VERIFICATION FAILED - backing view {}.{} not found in pg_class after spi_run_ddl",
                    schema_name,
                    view_name
                );
                false
            }
        }
        Err(e) => {
            // Log verification query failure
            notice!(
                "DEBUG: verification query FAILED - could not check pg_class: {}",
                e
            );
            // Note: error!() macro diverges, so this return is unreachable but needed for type checking
            return Err(TViewError::SpiError {
                query: format!("Check view {schema_name}.{view_name} exists"),
                error: e.to_string(),
            });
        }
    };

    if !exists {
        return Err(TViewError::CatalogError {
            operation: format!("Create view {schema_name}.{view_name}"),
            pg_error: "View was not created (CREATE VIEW succeeded but view missing from pg_class)"
                .to_string(),
        });
    }

    Ok(())
}

/// Map a scalar `PostgreSQL` type name to an uppercase SQL type string for CREATE TABLE.
///
/// Handles both `information_schema.data_type` values (e.g. `"boolean"`, `"uuid"`) and
/// `udt_name` values for extension types (e.g. `"ltree"`, `"geometry"`).
/// Unknown names fall back to `TEXT`.
fn scalar_pg_type_to_sql(pg_type: &str) -> &'static str {
    match pg_type {
        // ── Boolean ───────────────────────────────────────────────────────────
        "boolean" => "BOOLEAN",
        // ── UUID ──────────────────────────────────────────────────────────────
        "uuid" => "UUID",
        // ── JSON / JSONB ───────────────────────────────────────────────────────
        "jsonb" => "JSONB",
        "json" => "JSON",
        // ── Exact numerics ────────────────────────────────────────────────────
        "bigint" | "int8" => "BIGINT",
        "integer" | "int4" | "int" => "INTEGER",
        "smallint" | "int2" => "SMALLINT",
        "numeric" | "decimal" => "NUMERIC",
        // ── Floating point ────────────────────────────────────────────────────
        "real" | "float4" => "REAL",
        "double precision" | "float8" => "DOUBLE PRECISION",
        // ── Date / time ───────────────────────────────────────────────────────
        "timestamp with time zone" | "timestamptz" => "TIMESTAMPTZ",
        "timestamp without time zone" | "timestamp" => "TIMESTAMP",
        "date" => "DATE",
        "time with time zone" | "timetz" => "TIMETZ",
        "time without time zone" | "time" => "TIME",
        "interval" => "INTERVAL",
        // ── Network / binary ─────────────────────────────────────────────────
        "inet" => "INET",
        "cidr" => "CIDR",
        "macaddr" => "MACADDR",
        "macaddr8" => "MACADDR8",
        "bytea" => "BYTEA",
        // ── Text search ───────────────────────────────────────────────────────
        "tsvector" => "TSVECTOR",
        "tsquery" => "TSQUERY",
        // ── Range types ───────────────────────────────────────────────────────
        "int4range" => "INT4RANGE",
        "int8range" => "INT8RANGE",
        "numrange" => "NUMRANGE",
        "tsrange" => "TSRANGE",
        "tstzrange" => "TSTZRANGE",
        "daterange" => "DATERANGE",
        // ── Built-in geometric ────────────────────────────────────────────────
        "point" => "POINT",
        "line" => "LINE",
        "lseg" => "LSEG",
        "box" => "BOX",
        "path" => "PATH",
        "polygon" => "POLYGON",
        "circle" => "CIRCLE",
        // ── Extension / user-defined types (udt_name, e.g. ltree, geometry) ──
        // Known extension types — resolved via PostgreSQL search_path at runtime.
        "ltree" => "LTREE",
        "lquery" => "LQUERY",
        "ltxtquery" => "LTXTQUERY",
        "geometry" => "GEOMETRY",
        "geography" => "GEOGRAPHY",
        "hstore" => "HSTORE",
        "citext" => "CITEXT",
        // ── Other ─────────────────────────────────────────────────────────────
        "money" => "MONEY",
        "bit" => "BIT",
        "bit varying" => "BIT VARYING",
        "xml" => "XML",
        // text, character varying, character, unknown → TEXT
        _ => "TEXT",
    }
}

/// Resolve an `information_schema.columns` row (`data_type` + `udt_name`) to a SQL type
/// string suitable for a CREATE TABLE column definition.
///
/// Handles the three cases `PostgreSQL` presents:
/// - Built-in scalar: `data_type` is the canonical name, `udt_name` is redundant.
/// - Extension / user-defined: `data_type = "USER-DEFINED"`, `udt_name` is the type name.
/// - Array: `data_type = "ARRAY"`, `udt_name` is `_<element_udt_name>` (e.g. `_uuid`).
fn resolve_pg_column_type(data_type: &str, udt_name: Option<&str>) -> String {
    match data_type {
        "USER-DEFINED" => {
            // udt_name holds the extension type name (ltree, geometry, hstore, citext, …)
            udt_name.map_or_else(
                || "TEXT".to_string(),
                |u| scalar_pg_type_to_sql(u).to_string(),
            )
        }
        "ARRAY" => {
            // udt_name is "_<element>" (e.g. "_uuid" → UUID[], "_ltree" → LTREE[])
            let element_sql = udt_name
                .and_then(|u| u.strip_prefix('_'))
                .map_or("TEXT", scalar_pg_type_to_sql);
            format!("{element_sql}[]")
        }
        other => scalar_pg_type_to_sql(other).to_string(),
    }
}

/// Create the materialized table with proper schema inferred from the backing view
fn create_materialized_table(
    tview_name: &str,
    schema: &TViewSchema,
    schema_name: &str,
    distinct_on_output_keys: &[String],
) -> TViewResult<()> {
    let qi_schema = quote_identifier(schema_name);
    let qi_tview = quote_identifier(tview_name);

    // For DISTINCT ON TVIEWs the dedup key is the table PK; pk_<entity> becomes a plain
    // column. The key here is the projected OUTPUT column (the name the view exposes).
    let first_dedup = distinct_on_output_keys.first().map(String::as_str);
    let is_distinct_on = first_dedup.is_some();

    // Build column definitions based on inferred schema
    let mut columns = Vec::new();

    // Primary key column
    if let Some(pk) = &schema.pk_column {
        if is_distinct_on && first_dedup != Some(pk.as_str()) {
            // DISTINCT ON TVIEW: pk_<entity> is a plain column, not the table PK
            columns.push(format!("{} BIGINT", quote_identifier(pk)));
        } else if is_distinct_on {
            // pk_<entity> is itself the dedup key — make it the PK
            columns.push(format!("{} BIGINT PRIMARY KEY", quote_identifier(pk)));
        } else {
            columns.push(format!("{} BIGINT PRIMARY KEY", quote_identifier(pk)));
        }
    }

    // ID column (Trinity identifier)
    if let Some(id) = &schema.id_column {
        if first_dedup == Some(id.as_str()) {
            // Dedup key on `id` — make it the table PK with a UNIQUE constraint
            columns.push(format!("{} UUID PRIMARY KEY", quote_identifier(id)));
        } else {
            columns.push(format!("{} UUID NOT NULL", quote_identifier(id)));
        }
    }

    // Identifier column (optional Trinity identifier)
    if let Some(identifier) = &schema.identifier_column {
        columns.push(format!("{} TEXT", quote_identifier(identifier)));
    }

    // Data column (JSONB read model)
    if let Some(data) = &schema.data_column {
        columns.push(format!("{} JSONB", quote_identifier(data)));
    }

    // Foreign key columns (for lineage tracking)
    for fk in &schema.fk_columns {
        columns.push(format!("{} BIGINT", quote_identifier(fk)));
    }

    // Fetch actual column types from the backing view to guard against name-based
    // type mismatches. A column ending in `_id` is inferred as UUID by infer_schema,
    // but it may actually be TEXT (e.g. customer_contract_id, provider_contract_id).
    let entity = tview_name.strip_prefix("tv_").unwrap_or(tview_name);
    let view_name_for_types = format!("v_{entity}");
    let actual_col_types: std::collections::HashMap<String, String> = {
        // SAFETY: DatumWithOid::new wraps PostgreSQL datum pointers for SPI parameter passing.
        // The view/schema names are validated before this point.
        let args = vec![
            unsafe {
                DatumWithOid::new(
                    view_name_for_types.as_str(),
                    PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
                )
            },
            unsafe {
                DatumWithOid::new(schema_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
            },
        ];
        Spi::connect(|client| {
            let rows = client.select(
                "SELECT column_name::text, data_type::text, udt_name::text \
                 FROM information_schema.columns \
                 WHERE table_name = $1 AND table_schema = $2",
                None,
                &args,
            )?;
            let mut map = std::collections::HashMap::new();
            for row in rows {
                let name = row[1].value::<String>()?;
                let data_type = row[2].value::<String>()?;
                let udt_name = row[3].value::<String>()?;
                if let Some(n) = name {
                    // Resolve to the final SQL type string immediately so call sites
                    // can use the value directly without further translation.
                    let sql_type = resolve_pg_column_type(
                        data_type.as_deref().unwrap_or("text"),
                        udt_name.as_deref(),
                    );
                    map.insert(n, sql_type);
                }
            }
            Ok::<std::collections::HashMap<String, String>, pgrx::spi::Error>(map)
        })
        .unwrap_or_default()
    };

    // UUID foreign key columns (for filtering)
    // Verify the actual view column type — a column ending in _id may be TEXT
    // (e.g. customer_contract_id, provider_contract_id stored as TEXT, not UUID).
    for uuid_fk in &schema.uuid_fk_columns {
        let sql_type = actual_col_types
            .get(uuid_fk.as_str())
            .map_or("UUID", String::as_str); // if not found, trust name-based inference
        columns.push(format!("{} {sql_type}", quote_identifier(uuid_fk)));
    }

    // Additional columns: prefer the actual view column type over name-based inference.
    // Fixes mismatches like BOOLEAN inferred as TEXT, DATE inferred as TEXT, UUID[]
    // inferred as TEXT, LTREE inferred as TEXT, etc.
    for (col_name, col_type) in &schema.additional_columns_with_types {
        let effective_type = actual_col_types
            .get(col_name.as_str())
            .map_or(col_type.as_str(), String::as_str);
        let qi_col = quote_identifier(col_name);
        if first_dedup == Some(col_name.as_str()) {
            columns.push(format!("{qi_col} {effective_type} PRIMARY KEY"));
        } else {
            columns.push(format!("{qi_col} {effective_type}"));
        }
    }

    // Add timestamps for tracking
    columns.push("created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());
    columns.push("updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());

    let columns_sql = columns.join(",\n    ");

    let unlogged_keyword = if crate::config::unlogged_by_default() {
        "UNLOGGED "
    } else {
        ""
    };
    let create_table_sql =
        format!("CREATE {unlogged_keyword}TABLE {qi_schema}.{qi_tview} (\n    {columns_sql}\n)");

    crate::utils::spi_run_ddl(&create_table_sql).map_err(|e| TViewError::SpiError {
        query: create_table_sql,
        error: e,
    })?;

    // Create indexes for performance
    create_tview_indexes(tview_name, schema, schema_name)?;

    Ok(())
}

/// Create indexes on the materialized table for optimal query performance
fn create_tview_indexes(
    tview_name: &str,
    schema: &TViewSchema,
    schema_name: &str,
) -> TViewResult<()> {
    let qi_schema = quote_identifier(schema_name);
    let qi_tview = quote_identifier(tview_name);

    // Index on ID column (Trinity identifier)
    if let Some(id) = &schema.id_column {
        let idx_name = format!("idx_{tview_name}_{id}");
        let create_idx = format!(
            "CREATE INDEX {} ON {qi_schema}.{qi_tview} ({})",
            quote_identifier(&idx_name),
            quote_identifier(id),
        );
        crate::utils::spi_run_ddl(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e,
        })?;
    }

    // Index on UUID foreign key columns
    for uuid_fk in &schema.uuid_fk_columns {
        let idx_name = format!("idx_{tview_name}_{uuid_fk}");
        let create_idx = format!(
            "CREATE INDEX {} ON {qi_schema}.{qi_tview} ({})",
            quote_identifier(&idx_name),
            quote_identifier(uuid_fk),
        );
        crate::utils::spi_run_ddl(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e,
        })?;
    }

    // Index on data column if it exists (for JSONB queries)
    if let Some(data) = &schema.data_column {
        let idx_name = format!("idx_{tview_name}_{data}_gin");
        let create_idx = format!(
            "CREATE INDEX {} ON {qi_schema}.{qi_tview} USING GIN ({})",
            quote_identifier(&idx_name),
            quote_identifier(data),
        );
        crate::utils::spi_run_ddl(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e,
        })?;
    }

    Ok(())
}

/// Populate the materialized table with initial data from the backing view
fn populate_initial_data(
    tview_name: &str,
    view_name: &str,
    _schema: &TViewSchema,
    schema_name: &str,
) -> TViewResult<()> {
    // Get actual column names from the backing view (like pg_tviews_refresh does)
    // This ensures consistency and handles any discrepancies between inferred schema and actual view
    let view_oid = Spi::get_one::<Oid>(&format!(
        "SELECT c.oid FROM pg_class c JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname::text = '{view_name}' AND n.nspname::text = '{schema_name}' AND c.relkind = 'v'"
    ))?
    .ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find view {view_name} in schema {schema_name}"),
        pg_error: "View not found".to_string(),
    })?;

    let view_columns = crate::utils::get_view_columns_by_oid(view_oid)?;

    if view_columns.is_empty() {
        return Err(TViewError::CatalogError {
            operation: format!("Get columns for view {view_name}"),
            pg_error: "View has no selectable columns".to_string(),
        });
    }

    // Use the actual view columns for both insert and select
    let insert_columns = view_columns;

    let qi_schema = quote_identifier(schema_name);
    let qi_tview = quote_identifier(tview_name);
    let qi_view = quote_identifier(view_name);
    let col_list = insert_columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");

    let insert_sql = format!(
        "INSERT INTO {qi_schema}.{qi_tview} ({col_list}) \
         SELECT {col_list} FROM {qi_schema}.{qi_view}"
    );

    Spi::run(&insert_sql).map_err(|e| TViewError::SpiError {
        query: insert_sql,
        error: e.to_string(),
    })?;

    Ok(())
}

/// Quote a string for use in a `PostgreSQL` array literal.
///
/// Empty strings and strings containing special characters must be double-quoted
/// to avoid producing invalid array literals like `'{,}'`.
fn pg_array_elem(s: &str) -> String {
    if s.is_empty() || s.contains([',', '"', '\\', '{', '}', ' ']) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Register the TVIEW in metadata tables
#[allow(clippy::too_many_arguments)] // Reason: all args are distinct registration fields with no natural grouping
fn register_metadata(
    entity_name: &str,
    view_name: &str,
    tview_name: &str,
    definition_sql: &str,
    schema: &TViewSchema,
    cascade_paths: &[cascade_path::CascadePath],
    schema_name: &str,
    distinct_on_keys: &[String],
    distinct_on_output_keys: &[String],
) -> TViewResult<()> {
    // Detect whether the definition is a UNION / UNION ALL query.
    // CTE bodies are inside (...) so their UNION is at depth > 0 and not matched.
    let is_union = {
        let sql_lower = definition_sql.to_lowercase();
        crate::schema::parser::find_outer_union(&sql_lower, 0).is_some()
    };

    // Analyze dependencies to populate type/path/match_key info
    let dep_infos = analyze_dependencies(definition_sql, &schema.fk_columns);

    // Extract the direct-patch column→key map (issue #56): base columns that map
    // identity-style to top-level keys of this entity's own `data` object. Empty
    // ⇒ the direct-patch fast path never engages for this entity.
    let direct_map = extract_direct_column_map(definition_sql, &format!("tb_{entity_name}"));
    let direct_map_columns = direct_map
        .iter()
        .map(|(col, _)| pg_array_elem(col))
        .collect::<Vec<_>>()
        .join(",");
    let direct_map_keys = direct_map
        .iter()
        .map(|(_, key)| pg_array_elem(key))
        .collect::<Vec<_>>()
        .join(",");

    // Serialize schema information (quoted for safe PostgreSQL array literals)
    let fk_columns = schema
        .fk_columns
        .iter()
        .map(|s| pg_array_elem(s))
        .collect::<Vec<_>>()
        .join(",");
    let uuid_fk_columns = schema
        .uuid_fk_columns
        .iter()
        .map(|s| pg_array_elem(s))
        .collect::<Vec<_>>()
        .join(",");

    // Serialize dependency types
    let dep_types = dep_infos
        .iter()
        .map(|d| pg_array_elem(d.dep_type.as_str()))
        .collect::<Vec<_>>()
        .join(",");

    // Serialize dependency paths (TEXT[] format, empty string for None)
    let dep_paths = dep_infos
        .iter()
        .map(|d| {
            pg_array_elem(
                &d.jsonb_path
                    .as_ref()
                    .map_or_else(String::new, |path| path.join(".")),
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    // Serialize array match keys (empty string for None)
    let array_keys = dep_infos
        .iter()
        .map(|d| pg_array_elem(&d.array_match_key.clone().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(",");

    // Serialize cascade paths as a TEXT[] array of JSON strings
    let cascade_paths_str = cascade_paths
        .iter()
        .map(|path| {
            let json = serde_json::to_string(path).expect("Failed to serialize cascade path");
            pg_array_elem(&json)
        })
        .collect::<Vec<_>>()
        .join(",");
    let cascade_paths_literal = format!("'{{{cascade_paths_str}}}'");

    // Get OIDs for the created objects (schema-qualified, parameterized to prevent injection)
    let view_oid_args = vec![
        unsafe { DatumWithOid::new(view_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe { DatumWithOid::new(schema_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
    ];
    let view_oid_result = Spi::get_one_with_args::<pg_sys::Oid>(
        "SELECT c.oid FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = $1 AND n.nspname = $2 AND c.relkind = 'v'",
        &view_oid_args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for view {schema_name}.{view_name}"),
        pg_error: e.to_string(),
    })?;

    let table_oid_args = vec![
        unsafe { DatumWithOid::new(tview_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe { DatumWithOid::new(schema_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
    ];
    let table_oid_result = Spi::get_one_with_args::<pg_sys::Oid>(
        "SELECT c.oid FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = $1 AND n.nspname = $2 AND c.relkind = 'r'",
        &table_oid_args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for table {schema_name}.{tview_name}"),
        pg_error: e.to_string(),
    })?;

    let view_oid = view_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find view {schema_name}.{view_name}"),
        pg_error: "View OID not found".to_string(),
    })?;

    let table_oid = table_oid_result.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Find table {schema_name}.{tview_name}"),
        pg_error: "Table OID not found".to_string(),
    })?;

    // Serialize distinct_on_keys (source) and distinct_on_output_keys as array literals
    let distinct_on_str = distinct_on_keys
        .iter()
        .map(|s| pg_array_elem(s))
        .collect::<Vec<_>>()
        .join(",");
    let distinct_on_output_str = distinct_on_output_keys
        .iter()
        .map(|s| pg_array_elem(s))
        .collect::<Vec<_>>()
        .join(",");

    // Insert metadata record (entity + definition parameterized; OIDs and array literals are safe internal values)
    let insert_meta_sql = format!(
        "INSERT INTO pg_tview_meta (
            entity,
            view_oid,
            table_oid,
            definition,
            cascade_paths,
            fk_columns,
            uuid_fk_columns,
            dependency_types,
            dependency_paths,
            array_match_keys,
            distinct_on_keys,
            distinct_on_output_keys,
            direct_map_columns,
            direct_map_keys,
            is_union
        ) VALUES ($1, {}, {}, $2, {}, '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}', '{{{}}}', {})
        ON CONFLICT (entity) DO NOTHING",
        view_oid.to_u32(),
        table_oid.to_u32(),
        cascade_paths_literal,
        fk_columns,
        uuid_fk_columns,
        dep_types,
        dep_paths,
        array_keys,
        distinct_on_str,
        distinct_on_output_str,
        direct_map_columns,
        direct_map_keys,
        is_union
    );

    let args = [
        unsafe { DatumWithOid::new(entity_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe {
            DatumWithOid::new(
                definition_sql,
                PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
            )
        },
    ];
    Spi::run_with_args(&insert_meta_sql, &args).map_err(|e| TViewError::SpiError {
        query: insert_meta_sql,
        error: e.to_string(),
    })?;

    Ok(())
}

/// Transform a raw SELECT statement into TVIEW format
///
/// Takes a simple SELECT like "SELECT id, name, price FROM `tb_product`"
/// and transforms it into a proper TVIEW format with:
/// - pk_<entity> column (generated from the source table's primary key or id column)
/// - id column (UUID, generated from the source table's primary key)
/// - data column (JSONB with all fields)
///
/// This creates a "prepared view" that wraps the raw SELECT with TVIEW conventions.
fn transform_raw_select_to_tview(
    entity_name: &str,
    select_sql: &str,
) -> TViewResult<(String, TViewSchema)> {
    // Create a temporary view to analyze the raw SELECT
    let temp_view_name = format!("_temp_raw_{entity_name}");
    let qi_temp_view = quote_identifier(&temp_view_name);

    // First, create temp view to analyze columns
    let create_temp = format!("CREATE TEMP VIEW {qi_temp_view} AS {select_sql}");

    crate::utils::spi_run_ddl(&create_temp).map_err(|e| TViewError::SpiError {
        query: create_temp.clone(),
        error: e,
    })?;

    // Get columns from temp view (parameterized lookup)
    // Cast to text to avoid sql_identifier domain type issues
    let get_columns_sql = "SELECT column_name::text, data_type::text
         FROM information_schema.columns
         WHERE table_name = $1
         ORDER BY ordinal_position";

    let temp_view_args = vec![unsafe {
        DatumWithOid::new(
            temp_view_name.as_str(),
            PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
        )
    }];
    let columns: Vec<(String, String)> = Spi::connect(|client| {
        let rows = client.select(get_columns_sql, None, &temp_view_args)?;
        let mut result = Vec::new();
        for row in rows {
            let col_name: String = row[1].value()?.ok_or_else(|| {
                spi::Error::from(crate::TViewError::SpiError {
                    query: get_columns_sql.to_string(),
                    error: "column name is NULL".to_string(),
                })
            })?;
            let data_type: String = row[2].value()?.ok_or_else(|| {
                spi::Error::from(crate::TViewError::SpiError {
                    query: get_columns_sql.to_string(),
                    error: "data type is NULL".to_string(),
                })
            })?;
            result.push((col_name, data_type));
        }
        Ok(result)
    })
    .map_err(|e: spi::Error| TViewError::CatalogError {
        operation: "Get columns from temp view".to_string(),
        pg_error: format!("{e:?}"),
    })?;

    // Drop temp view
    crate::utils::spi_run_ddl(&format!("DROP VIEW {qi_temp_view}")).ok();

    // Find primary key column using this priority order:
    // 1. Column named exactly 'pk' (original source PK, most reliable)
    // 2. Column that is integer/serial type (natural database PKs)
    // 3. Column named 'id' (may be UUID identifier, fallback only)
    // This avoids incorrectly selecting a UUID 'id' column when an actual 'pk' exists.
    let pk_source_col = columns
        .iter()
        .find(|(name, _)| name == "pk")
        .or_else(|| {
            columns
                .iter()
                .find(|(_, typ)| typ.contains("int") || typ.contains("serial"))
        })
        .or_else(|| columns.iter().find(|(name, _)| name == "id"))
        .map(|(name, _)| name.clone())
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: select_sql.to_string(),
            reason: "No suitable primary key column found (need 'pk', an integer column, or 'id')"
                .to_string(),
        })?;

    // Build explicit column lists for clarity and control

    // 1. Build the source column list (from the subquery)
    let _source_columns: Vec<String> = columns
        .iter()
        .map(|(name, _)| format!("source.{name}"))
        .collect();

    // 2. Build JSONB data column pairs explicitly
    let data_columns: Vec<String> = columns
        .iter()
        .map(|(name, _)| format!("'{name}', source.{name}"))
        .collect();

    // 3. Generate transformed SELECT with explicit column references
    // This makes it clear exactly what's being selected and how it's transformed
    let transformed_select = format!(
        "SELECT
            source.{} AS pk_{},
            gen_random_uuid() AS id,
            jsonb_build_object({}) AS data
        FROM ({}) AS source",
        pk_source_col,
        entity_name,
        data_columns.join(", "),
        select_sql
    );

    // Infer schema from transformed SELECT
    let schema = infer_schema(&transformed_select)?;

    Ok((transformed_select, schema))
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    // ── Unit tests for type resolution (no database required) ──────────────────

    #[test]
    fn test_scalar_pg_type_boolean() {
        assert_eq!(super::scalar_pg_type_to_sql("boolean"), "BOOLEAN");
    }

    #[test]
    fn test_scalar_pg_type_uuid() {
        assert_eq!(super::scalar_pg_type_to_sql("uuid"), "UUID");
    }

    #[test]
    fn test_scalar_pg_type_numeric() {
        assert_eq!(super::scalar_pg_type_to_sql("bigint"), "BIGINT");
        assert_eq!(super::scalar_pg_type_to_sql("int8"), "BIGINT");
        assert_eq!(super::scalar_pg_type_to_sql("integer"), "INTEGER");
        assert_eq!(super::scalar_pg_type_to_sql("int4"), "INTEGER");
        assert_eq!(super::scalar_pg_type_to_sql("smallint"), "SMALLINT");
        assert_eq!(super::scalar_pg_type_to_sql("numeric"), "NUMERIC");
        assert_eq!(super::scalar_pg_type_to_sql("decimal"), "NUMERIC");
    }

    #[test]
    fn test_scalar_pg_type_floating_point() {
        assert_eq!(super::scalar_pg_type_to_sql("real"), "REAL");
        assert_eq!(super::scalar_pg_type_to_sql("float4"), "REAL");
        assert_eq!(
            super::scalar_pg_type_to_sql("double precision"),
            "DOUBLE PRECISION"
        );
        assert_eq!(super::scalar_pg_type_to_sql("float8"), "DOUBLE PRECISION");
    }

    #[test]
    fn test_scalar_pg_type_temporal() {
        assert_eq!(
            super::scalar_pg_type_to_sql("timestamp with time zone"),
            "TIMESTAMPTZ"
        );
        assert_eq!(
            super::scalar_pg_type_to_sql("timestamp without time zone"),
            "TIMESTAMP"
        );
        assert_eq!(super::scalar_pg_type_to_sql("date"), "DATE");
        assert_eq!(super::scalar_pg_type_to_sql("time"), "TIME");
    }

    #[test]
    fn test_scalar_pg_type_json() {
        assert_eq!(super::scalar_pg_type_to_sql("jsonb"), "JSONB");
        assert_eq!(super::scalar_pg_type_to_sql("json"), "JSON");
    }

    #[test]
    fn test_scalar_pg_type_extensions() {
        assert_eq!(super::scalar_pg_type_to_sql("ltree"), "LTREE");
        assert_eq!(super::scalar_pg_type_to_sql("lquery"), "LQUERY");
        assert_eq!(super::scalar_pg_type_to_sql("geometry"), "GEOMETRY");
        assert_eq!(super::scalar_pg_type_to_sql("geography"), "GEOGRAPHY");
        assert_eq!(super::scalar_pg_type_to_sql("hstore"), "HSTORE");
        assert_eq!(super::scalar_pg_type_to_sql("citext"), "CITEXT");
    }

    #[test]
    fn test_scalar_pg_type_unknown_fallback() {
        assert_eq!(super::scalar_pg_type_to_sql("unknown_type"), "TEXT");
        assert_eq!(super::scalar_pg_type_to_sql(""), "TEXT");
    }

    #[test]
    fn test_resolve_pg_column_type_builtin_scalar() {
        assert_eq!(super::resolve_pg_column_type("boolean", None), "BOOLEAN");
        assert_eq!(super::resolve_pg_column_type("uuid", None), "UUID");
        assert_eq!(super::resolve_pg_column_type("bigint", None), "BIGINT");
        assert_eq!(super::resolve_pg_column_type("text", None), "TEXT");
    }

    #[test]
    fn test_resolve_pg_column_type_user_defined() {
        assert_eq!(
            super::resolve_pg_column_type("USER-DEFINED", Some("ltree")),
            "LTREE"
        );
        assert_eq!(
            super::resolve_pg_column_type("USER-DEFINED", Some("geometry")),
            "GEOMETRY"
        );
        assert_eq!(
            super::resolve_pg_column_type("USER-DEFINED", Some("hstore")),
            "HSTORE"
        );
    }

    #[test]
    fn test_resolve_pg_column_type_user_defined_unknown() {
        // Unknown USER-DEFINED type falls back to TEXT
        assert_eq!(
            super::resolve_pg_column_type("USER-DEFINED", Some("custom_type")),
            "TEXT"
        );
    }

    #[test]
    fn test_resolve_pg_column_type_user_defined_missing_udt_name() {
        // USER-DEFINED without udt_name falls back to TEXT
        assert_eq!(super::resolve_pg_column_type("USER-DEFINED", None), "TEXT");
    }

    #[test]
    fn test_resolve_pg_column_type_array_uuid() {
        assert_eq!(
            super::resolve_pg_column_type("ARRAY", Some("_uuid")),
            "UUID[]"
        );
    }

    #[test]
    fn test_resolve_pg_column_type_array_text() {
        assert_eq!(
            super::resolve_pg_column_type("ARRAY", Some("_text")),
            "TEXT[]"
        );
    }

    #[test]
    fn test_resolve_pg_column_type_array_integer() {
        assert_eq!(
            super::resolve_pg_column_type("ARRAY", Some("_int4")),
            "INTEGER[]"
        );
    }

    #[test]
    fn test_resolve_pg_column_type_array_ltree() {
        // Array of extension types: _ltree → LTREE[]
        assert_eq!(
            super::resolve_pg_column_type("ARRAY", Some("_ltree")),
            "LTREE[]"
        );
    }

    #[test]
    fn test_resolve_pg_column_type_array_missing_udt_name() {
        // ARRAY without udt_name falls back to TEXT[]
        assert_eq!(super::resolve_pg_column_type("ARRAY", None), "TEXT[]");
    }

    #[test]
    fn test_resolve_pg_column_type_array_unknown_element() {
        // Array of unknown type: _unknown → TEXT[]
        assert_eq!(
            super::resolve_pg_column_type("ARRAY", Some("_unknown")),
            "TEXT[]"
        );
    }

    // ── Integration tests requiring database access ───────────────────────────

    #[test]
    fn test_tview_exists_non_existent() {
        // Compile-time check only — live DB tests use #[pg_test] below
    }

    /// TVIEW objects are created in the schema that is first in `search_path`,
    /// not hardcoded to public.
    #[pg_test]
    fn test_create_tview_respects_search_path() {
        Spi::run("CREATE SCHEMA tview_test_ns").unwrap();
        Spi::run("SET search_path TO tview_test_ns, public").unwrap();
        Spi::run("CREATE TABLE tb_item (pk_item BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_item VALUES (1, 'Widget')").unwrap();

        Spi::run(
            "SELECT pg_tviews_create('item', $$
            SELECT pk_item, jsonb_build_object('name', name) AS data
            FROM tb_item
        $$)",
        )
        .unwrap();

        // tv_item must be in the target schema
        let in_target = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_item' AND n.nspname = 'tview_test_ns'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(in_target, "tv_item should be in tview_test_ns, not public");

        // tv_item must NOT leak into public
        let in_public = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_item' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(!in_public, "tv_item must not be created in public schema");

        // The backing view v_item must be in the same schema
        let view_in_target = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'v_item' AND n.nspname = 'tview_test_ns'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(view_in_target, "v_item should be in tview_test_ns");
    }

    /// With the default `search_path`, objects still land in public (regression guard).
    #[pg_test]
    fn test_create_tview_defaults_to_public() {
        Spi::run("SET search_path TO public").unwrap();
        Spi::run("CREATE TABLE tb_gadget (pk_gadget BIGSERIAL PRIMARY KEY, label TEXT)").unwrap();
        Spi::run("INSERT INTO tb_gadget VALUES (1, 'Gizmo')").unwrap();

        Spi::run(
            "SELECT pg_tviews_create('gadget', $$
            SELECT pk_gadget, jsonb_build_object('label', label) AS data
            FROM tb_gadget
        $$)",
        )
        .unwrap();

        let in_public = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_gadget' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            in_public,
            "tv_gadget should be in public with default search_path"
        );
    }

    /// Test CTAS (CREATE TABLE AS SELECT) with preexisting data.
    /// This reproduces the bug where initial population fails.
    #[pg_test]
    fn test_ctas_with_preexisting_data() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_ctas_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_ctas_test VALUES (1, 'Alice'), (2, 'Bob')").unwrap();

        // Do CTAS - this should create a TVIEW with the existing data
        Spi::run(
            "CREATE TABLE tv_ctas_test AS
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_ctas_test",
        )
        .unwrap();

        // Check that TVIEW was created
        let tview_exists = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_ctas_test' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(tview_exists, "tv_ctas_test should exist");

        // Check that it has the initial data (this is where the bug manifests)
        let row_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_ctas_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            row_count, 2,
            "tv_ctas_test should have 2 rows from initial population"
        );

        // Check specific data
        let alice_exists = Spi::get_one::<bool>(
            "SELECT COUNT(*) > 0 FROM tv_ctas_test WHERE data->>'name' = 'Alice'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(alice_exists, "Alice should be in the TVIEW");
    }

    /// Test that TVIEW tables respect the `unlogged_by_default` GUC.
    #[pg_test]
    fn test_tview_unlogged_guc_control() {
        Spi::run("SET search_path TO public").unwrap();

        // Test with GUC set to true (default)
        Spi::run("SET pg_tviews.unlogged_by_default TO true").unwrap();

        // Create base table
        Spi::run("CREATE TABLE tb_guc_test1 (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();

        // Create TVIEW
        Spi::run(
            "SELECT pg_tviews_create('guc_test1', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_guc_test1
        $$)",
        )
        .unwrap();

        // Check that the TVIEW table is UNLOGGED
        let is_unlogged = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'u' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_guc_test1' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            is_unlogged,
            "tv_guc_test1 should be UNLOGGED when GUC is true"
        );

        // Test with GUC set to false
        Spi::run("SET pg_tviews.unlogged_by_default TO false").unwrap();

        // Create another base table
        Spi::run("CREATE TABLE tb_guc_test2 (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();

        // Create another TVIEW
        Spi::run(
            "SELECT pg_tviews_create('guc_test2', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_guc_test2
        $$)",
        )
        .unwrap();

        // Check that this TVIEW table is LOGGED
        let is_logged = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'p' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_guc_test2' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(is_logged, "tv_guc_test2 should be LOGGED when GUC is false");

        // Reset GUC to default
        Spi::run("RESET pg_tviews.unlogged_by_default").unwrap();
    }

    /// Test ALTER TABLE SET UNLOGGED/LOGGED on TVIEWs.
    #[pg_test]
    fn test_alter_tview_unlogged_logged() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_alter_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_alter_test VALUES (1, 'Alice'), (2, 'Bob')").unwrap();

        // Create TVIEW as LOGGED first
        Spi::run("SET pg_tviews.unlogged_by_default TO false").unwrap();
        Spi::run(
            "SELECT pg_tviews_create('alter_test', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_alter_test
        $$)",
        )
        .unwrap();

        // Verify TVIEW is initially LOGGED
        let is_logged = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'p' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_alter_test' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(is_logged, "tv_alter_test should initially be LOGGED");

        // ALTER TABLE to UNLOGGED
        Spi::run("ALTER TABLE tv_alter_test SET UNLOGGED").unwrap();

        // Verify it's now UNLOGGED
        let is_unlogged = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'u' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_alter_test' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            is_unlogged,
            "tv_alter_test should be UNLOGGED after ALTER TABLE"
        );

        // ALTER TABLE back to LOGGED
        Spi::run("ALTER TABLE tv_alter_test SET LOGGED").unwrap();

        // Verify it's now LOGGED again
        let is_logged_again = Spi::get_one::<bool>(
            "SELECT c.relpersistence = 'p' FROM pg_class c \
             JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname = 'tv_alter_test' AND n.nspname = 'public'",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            is_logged_again,
            "tv_alter_test should be LOGGED again after ALTER TABLE"
        );

        // Reset GUC
        Spi::run("RESET pg_tviews.unlogged_by_default").unwrap();
    }

    /// Test data integrity during ALTER TABLE UNLOGGED/LOGGED operations.
    #[pg_test]
    fn test_alter_tview_data_integrity() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_integrity_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)")
            .unwrap();
        Spi::run("INSERT INTO tb_integrity_test VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Charlie')")
            .unwrap();

        // Create TVIEW as LOGGED
        Spi::run("SET pg_tviews.unlogged_by_default TO false").unwrap();
        Spi::run(
            "SELECT pg_tviews_create('integrity_test', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_integrity_test
        $$)",
        )
        .unwrap();

        // Verify data is present
        let initial_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_integrity_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(initial_count, 3, "TVIEW should have 3 rows initially");

        // ALTER TABLE from LOGGED to UNLOGGED - data should be preserved
        Spi::run("ALTER TABLE tv_integrity_test SET UNLOGGED").unwrap();

        let after_unlogged_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_integrity_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_unlogged_count, 3,
            "Data should be preserved when converting LOGGED to UNLOGGED"
        );

        // ALTER TABLE from UNLOGGED to LOGGED - data is preserved (PostgreSQL behavior)
        Spi::run("ALTER TABLE tv_integrity_test SET LOGGED").unwrap();

        let after_logged_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_integrity_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_logged_count, 3,
            "Data should be preserved when converting UNLOGGED to LOGGED"
        );

        // Reset GUC
        Spi::run("RESET pg_tviews.unlogged_by_default").unwrap();
    }

    /// Test detection of post-crash empty UNLOGGED table.
    #[pg_test]
    fn test_detect_post_crash_empty_tview() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_crash_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_crash_test VALUES (1, 'Alice'), (2, 'Bob')").unwrap();

        // Create TVIEW
        Spi::run(
            "SELECT pg_tviews_create('crash_test', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_crash_test
        $$)",
        )
        .unwrap();

        // Verify TVIEW has data
        let initial_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_crash_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(initial_count, 2, "TVIEW should have 2 rows initially");

        // Before truncation, should not detect crash
        let crash_before = crate::lifecycle::detect_post_crash_truncation("crash_test").unwrap();
        assert!(!crash_before, "Should not detect crash when table has data");

        // Simulate post-crash truncation (UNLOGGED table behavior)
        Spi::run("TRUNCATE TABLE tv_crash_test").unwrap();

        // Verify table is now empty
        let after_truncate_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_crash_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_truncate_count, 0,
            "TVIEW should be empty after truncate"
        );

        // Should now detect crash (table empty but view has data)
        let crash_detected = crate::lifecycle::detect_post_crash_truncation("crash_test").unwrap();
        assert!(
            crash_detected,
            "Should detect crash when UNLOGGED table is empty but view has data"
        );

        // Verify backing view still has data
        let view_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM v_crash_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            view_count, 2,
            "Backing view should still have data after table truncate"
        );
    }

    /// Test automatic recovery after crash detection.
    #[pg_test]
    fn test_auto_recover_after_crash() {
        Spi::run("SET search_path TO public").unwrap();

        // Create base table with data
        Spi::run("CREATE TABLE tb_recover_test (pk_test BIGSERIAL PRIMARY KEY, name TEXT)")
            .unwrap();
        Spi::run("INSERT INTO tb_recover_test VALUES (1, 'Alice'), (2, 'Bob')").unwrap();

        // Create TVIEW
        Spi::run(
            "SELECT pg_tviews_create('recover_test', $$
            SELECT pk_test, jsonb_build_object('name', name) AS data
            FROM tb_recover_test
        $$)",
        )
        .unwrap();

        // Verify TVIEW has data initially
        let initial_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_recover_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(initial_count, 2, "TVIEW should have 2 rows initially");

        // Simulate post-crash truncation
        Spi::run("TRUNCATE TABLE tv_recover_test").unwrap();

        // Verify table is now empty
        let after_truncate_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_recover_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_truncate_count, 0,
            "TVIEW should be empty after truncate"
        );

        // Call auto-recovery function
        let recovery_performed =
            Spi::get_one::<bool>("SELECT pg_tviews_recover_after_crash('recover_test')")
                .unwrap()
                .unwrap_or(false);
        assert!(
            recovery_performed,
            "Recovery should be performed when crash is detected"
        );

        // Verify TVIEW has data again after recovery
        let after_recovery_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_recover_test")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            after_recovery_count, 2,
            "TVIEW should have 2 rows after recovery"
        );

        // Call recovery again - should return false (no recovery needed)
        let second_recovery =
            Spi::get_one::<bool>("SELECT pg_tviews_recover_after_crash('recover_test')")
                .unwrap()
                .unwrap_or(true);
        assert!(
            !second_recovery,
            "Second recovery call should return false when no crash detected"
        );
    }
}
