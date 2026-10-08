use crate::error::{TViewError, TViewResult};
use crate::utils::quote_identifier;
use pgrx::prelude::*;

/// Row-level trigger function: enqueues refreshes.
const ROW_HANDLER: &str = "pg_tview_trigger_handler";
/// Statement-level trigger function: flushes the refresh queue.
const FLUSH_HANDLER: &str = "pg_tview_flush_trigger";
/// Statement-level trigger function over transition tables: maps the changed rows
/// of a `mapped` table to keys (ADR 0157).
const DELTA_HANDLER: &str = "pg_tview_delta_trigger";
/// Statement-level `TRUNCATE` trigger function: refreshes the whole TVIEW.
const TRUNCATE_HANDLER: &str = "pg_tview_truncate_trigger";

/// Transition tables of the delta triggers, as the handler reads them.
pub const OLD_TABLE: &str = "pg_tviews_old";
pub const NEW_TABLE: &str = "pg_tviews_new";

/// One trigger a base table gets for a TVIEW.
struct TriggerSpec {
    function: &'static str,
    /// `pg_trigger.tgtype`: what recognises an installed one.
    tgtype: i16,
    /// `AFTER <events> ON t [REFERENCING …] FOR EACH <level>`.
    clause: &'static str,
    /// Name tag; statement triggers fire in name order, so `delta` comes before `flush`.
    tag: &'static str,
}

const ROW: TriggerSpec = TriggerSpec {
    function: ROW_HANDLER,
    tgtype: 1 | 4 | 8 | 16,
    clause: "AFTER INSERT OR UPDATE OR DELETE ON {table} FOR EACH ROW",
    tag: "row",
};
const FLUSH: TriggerSpec = TriggerSpec {
    function: FLUSH_HANDLER,
    tgtype: 4 | 8 | 16,
    clause: "AFTER INSERT OR UPDATE OR DELETE ON {table} FOR EACH STATEMENT",
    tag: "flush",
};
const TRUNCATE: TriggerSpec = TriggerSpec {
    function: TRUNCATE_HANDLER,
    tgtype: 32,
    clause: "AFTER TRUNCATE ON {table} FOR EACH STATEMENT",
    tag: "truncate",
};
// A trigger with transition tables takes one event.
const DELTA_INSERT: TriggerSpec = TriggerSpec {
    function: DELTA_HANDLER,
    tgtype: 4,
    clause: "AFTER INSERT ON {table} REFERENCING NEW TABLE AS pg_tviews_new FOR EACH STATEMENT",
    tag: "delta_i",
};
const DELTA_UPDATE: TriggerSpec = TriggerSpec {
    function: DELTA_HANDLER,
    tgtype: 16,
    clause: "AFTER UPDATE ON {table} REFERENCING OLD TABLE AS pg_tviews_old \
             NEW TABLE AS pg_tviews_new FOR EACH STATEMENT",
    tag: "delta_u",
};
const DELTA_DELETE: TriggerSpec = TriggerSpec {
    function: DELTA_HANDLER,
    tgtype: 8,
    clause: "AFTER DELETE ON {table} REFERENCING OLD TABLE AS pg_tviews_old FOR EACH STATEMENT",
    tag: "delta_d",
};

/// What every member of a partitioned base table's tree gets, the root excepted.
/// PostgreSQL copies only row triggers onto partitions, and a statement trigger
/// fires only on the table the statement names: a statement writing to a
/// partition directly needs a flush trigger of its own, and a `TRUNCATE` of one
/// a truncate trigger.
const PARTITION_MEMBER: &[TriggerSpec] = &[FLUSH, TRUNCATE];

/// Which triggers a base table gets for a TVIEW, from how its writes map to keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerSet {
    /// The key is read off each changed row: row trigger.
    Row,
    /// A query maps the statement's changed rows: delta triggers. A partitioned
    /// table maps row by row: transition tables of its statement triggers would
    /// see only the rows of statements naming it, and its partitions get no copy
    /// of a trigger with transition tables.
    Delta,
    /// Another TVIEW's table: its refreshes are mapped by the delta
    /// triggers alone. They fire inside the flush, which drains what they queue;
    /// a flush or TRUNCATE trigger there would flush again from inside it.
    TviewDelta,
    /// Refreshing another TVIEW refreshes this one: nothing.
    None,
}

impl TriggerSet {
    const fn specs(self, partitioned: bool) -> &'static [TriggerSpec] {
        match (self, partitioned) {
            (Self::Row, _) | (Self::Delta, true) => &[ROW, FLUSH, TRUNCATE],
            (Self::Delta, false) => &[DELTA_INSERT, DELTA_UPDATE, DELTA_DELETE, FLUSH, TRUNCATE],
            (Self::TviewDelta, _) => &[DELTA_INSERT, DELTA_UPDATE, DELTA_DELETE],
            (Self::None, _) => &[],
        }
    }
}

/// Every base table of a TVIEW with the triggers it gets.
pub type TriggerPlan = Vec<(pg_sys::Oid, TriggerSet)>;

/// The trigger plan of a TVIEW reading `base_tables`, from its lineage.
pub fn trigger_plan(base_tables: &[pg_sys::Oid], lineage: &crate::lineage::Lineage) -> TriggerPlan {
    use crate::lineage::TableKind;
    // Other TVIEWs' tables the lineage maps: they are not base tables.
    let tview_tables = lineage.tables.iter().filter(|t| {
        t.tview.is_some() && matches!(t.kind, TableKind::Mapped | TableKind::AllKeys(_))
    });
    tview_tables
        .map(|t| (pg_sys::Oid::from(t.relid), TriggerSet::TviewDelta))
        .chain(base_tables.iter().map(|&oid| {
            let table = lineage.tables.iter().find(|t| t.relid == oid.to_u32());
            let set = match table.map(|t| &t.kind) {
                // REFRESH MATERIALIZED VIEW fires no trigger: the ProcessUtility
                // hook follows it.
                _ if table.is_some_and(|t| t.matview) => TriggerSet::None,
                Some(TableKind::Mapped | TableKind::AllKeys(_)) => TriggerSet::Delta,
                Some(TableKind::Propagated(_)) => TriggerSet::None,
                None if lineage.unread.contains(&oid.to_u32()) => TriggerSet::None,
                Some(TableKind::Local(_)) | None => TriggerSet::Row,
            };
            (oid, set)
        }))
        .collect()
}

/// Name of a trigger for `entity` on `schema.relname`:
/// `trg_tview_<tag>_<entity>_on_<schema>_<table>`, fitted to 63 bytes. The tag
/// comes right after the fixed prefix, so the row trigger of one entity never
/// has the name of another entity's flush trigger.
fn trigger_name(tag: &str, entity: &str, schema: &str, relname: &str) -> String {
    crate::utils::fit_identifier(format!("trg_tview_{tag}_{entity}_on_{schema}_{relname}"))
}

/// A `pg_tviews` trigger on a base table.
struct InstalledTrigger {
    table_oid: pg_sys::Oid,
    /// Quoted, schema-qualified table.
    table: String,
    trigger: String,
    function: String,
    tgtype: i16,
}

/// The `pg_tviews` triggers installed for `entity`, on `table_oid` only when given.
///
/// A trigger is recognised by its function (`tgfoid`) and the entity it carries
/// as its argument, not by its name, which a rename of the table or its schema
/// leaves stale.
fn entity_triggers(
    entity: &str,
    table_oid: Option<pg_sys::Oid>,
) -> TViewResult<Vec<InstalledTrigger>> {
    let query = format!(
        "SELECT pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname), \
                t.tgname::text, p.proname::text, t.tgrelid, t.tgtype \
         FROM pg_catalog.pg_trigger t \
         JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
         JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
           AND p.proname IN ('{ROW_HANDLER}', '{FLUSH_HANDLER}', '{DELTA_HANDLER}', \
                             '{TRUNCATE_HANDLER}') \
           AND t.tgparentid = 0 \
           AND t.tgnargs = 1 \
           AND t.tgargs = pg_catalog.convert_to($1, pg_catalog.getdatabaseencoding()) \
                          || pg_catalog.decode('00', 'hex') \
           AND ($2 IS NULL OR t.tgrelid = $2)",
        schema = crate::utils::ext_schema(),
    );
    Spi::connect(|client| {
        let args = [
            crate::utils::spi::text(entity),
            crate::utils::spi::oid(table_oid),
        ];
        let mut found = Vec::new();
        for row in client.select(&query, None, &args)? {
            if let (Some(table), Some(trigger), Some(function), Some(table_oid), Some(tgtype)) = (
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<String>(3)?,
                row.get::<pg_sys::Oid>(4)?,
                row.get::<i16>(5)?,
            ) {
                found.push(InstalledTrigger {
                    table_oid,
                    table,
                    trigger,
                    function,
                    tgtype,
                });
            }
        }
        Ok::<_, spi::Error>(found)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Find triggers of TVIEW {entity}"),
        pg_error: e.to_string(),
    })
}

/// What the health check finds wrong with `pg_tviews`' triggers, each
/// as `<trigger or entity> on <table>`.
#[derive(Default)]
pub struct TriggerProblems {
    /// Triggers whose entity is not registered or does not read their table.
    pub orphaned: Vec<String>,
    /// Tables a TVIEW reads, or their partitions, that lack one of its triggers.
    pub missing: Vec<String>,
    /// `pg_tviews` triggers without an entity argument (installed by an older
    /// release): `pg_tviews_reregister_all()` replaces them.
    pub untagged: Vec<String>,
}

/// Check `pg_tviews`' triggers against the tables each registered TVIEW reads
/// (`tviews.pg_tview_reads`: ordinary and partitioned tables reached from the
/// backing view through views, other TVIEWs' tables excepted) and the triggers
/// its lineage gives each table ([`TriggerSet`]). Every partition of
/// a partitioned table carrying a row trigger expects the `PARTITION_MEMBER`
/// triggers; the copies `PostgreSQL` makes of the row trigger are not counted.
///
/// # Errors
/// Returns an error if the catalog query fails.
pub fn trigger_problems() -> TViewResult<TriggerProblems> {
    let kind = crate::catalog::registered::mapping_kind_sql("m", "r.relid");
    let query = format!(
        "WITH ours AS ( \
             SELECT t.tgname, t.tgrelid, p.proname, \
                    CASE WHEN t.tgnargs = 1 THEN pg_catalog.convert_from( \
                        pg_catalog.substring(t.tgargs, 1, pg_catalog.length(t.tgargs) - 1), \
                        pg_catalog.getdatabaseencoding()) END AS entity \
             FROM pg_catalog.pg_trigger t \
             JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
             WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
               AND p.proname IN ('{ROW_HANDLER}', '{FLUSH_HANDLER}', '{DELTA_HANDLER}', \
                                 '{TRUNCATE_HANDLER}') \
               AND t.tgparentid = 0 \
         ), \
         reads AS ( \
             SELECT DISTINCT r.entity, r.relid, c.relkind, \
                    r.relid IN (SELECT table_oid::oid FROM {meta}) AS tview, \
                    {kind} AS kind \
             FROM {schema}.pg_tview_reads r \
             JOIN {meta} m ON m.entity = r.entity \
             JOIN pg_catalog.pg_class c ON c.oid = r.relid AND c.relkind IN ('r', 'p') \
         ), \
         planned AS ( \
             SELECT r.entity, r.relid, r.relkind, f.proname \
             FROM reads r \
             CROSS JOIN (VALUES ('{ROW_HANDLER}'), ('{FLUSH_HANDLER}'), ('{DELTA_HANDLER}'), \
                                ('{TRUNCATE_HANDLER}')) AS f(proname) \
             WHERE CASE \
                 WHEN r.tview THEN r.kind IN ('mapped', 'all_keys') \
                                   AND f.proname = '{DELTA_HANDLER}' \
                 WHEN r.kind = 'local' OR (r.kind IN ('mapped', 'all_keys') AND r.relkind = 'p') \
                     THEN f.proname IN ('{ROW_HANDLER}', '{FLUSH_HANDLER}', '{TRUNCATE_HANDLER}') \
                 WHEN r.kind IN ('mapped', 'all_keys') \
                     THEN f.proname IN ('{DELTA_HANDLER}', '{FLUSH_HANDLER}', '{TRUNCATE_HANDLER}') \
                 ELSE false END \
         ), \
         expected AS ( \
             SELECT entity, relid, proname FROM planned \
             UNION ALL \
             SELECT p.entity, m.relid::pg_catalog.oid, f.proname \
             FROM planned p \
             CROSS JOIN LATERAL pg_catalog.pg_partition_tree(p.relid) m \
             CROSS JOIN (VALUES ('{FLUSH_HANDLER}'), ('{TRUNCATE_HANDLER}')) AS f(proname) \
             WHERE p.relkind = 'p' AND p.proname = '{ROW_HANDLER}' AND m.relid <> p.relid \
         ) \
         SELECT 'orphaned', pg_catalog.format('%I on %s', o.tgname, \
                                              o.tgrelid::pg_catalog.regclass) \
         FROM ours o \
         WHERE o.entity IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM expected e \
                           WHERE e.entity = o.entity AND e.relid = o.tgrelid \
                             AND e.proname = o.proname) \
         UNION ALL \
         SELECT 'missing', pg_catalog.format('%s (%s) on %s', e.entity, e.proname, \
                                             e.relid::pg_catalog.regclass) \
         FROM expected e \
         WHERE NOT EXISTS (SELECT 1 FROM ours o \
                           WHERE o.entity = e.entity AND o.tgrelid = e.relid \
                             AND o.proname = e.proname) \
         UNION ALL \
         SELECT 'untagged', pg_catalog.format('%I on %s', o.tgname, \
                                              o.tgrelid::pg_catalog.regclass) \
         FROM ours o WHERE o.entity IS NULL \
         ORDER BY 1, 2",
        schema = crate::utils::ext_schema(),
        meta = crate::utils::meta_table(),
    );
    Spi::connect(|client| {
        let mut problems = TriggerProblems::default();
        for row in client.select(&query, None, &[])? {
            if let (Some(kind), Some(what)) = (row.get::<String>(1)?, row.get::<String>(2)?) {
                match kind.as_str() {
                    "orphaned" => problems.orphaned.push(what),
                    "missing" => problems.missing.push(what),
                    _ => problems.untagged.push(what),
                }
            }
        }
        Ok::<_, spi::Error>(problems)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Check pg_tviews triggers".to_string(),
        pg_error: e.to_string(),
    })
}

/// Install the triggers of `plan` for a TVIEW: on each base table those its
/// [`TriggerSet`] names, every one called schema-qualified with the entity as its
/// argument, which is how [`remove_entity_triggers`] finds them. A trigger already
/// there is left alone.
///
/// The row trigger enqueues the key read off each changed row; the delta triggers
/// map a statement's changed rows with the table's mapping query; the flush
/// trigger flushes the queue so auto-commit statements refresh too (the
/// `ProcessUtility` hook flushes on an explicit COMMIT); the TRUNCATE trigger
/// refreshes the whole TVIEW.
///
/// # Errors
/// Returns error if trigger creation or installation fails.
pub fn install_triggers(plan: &[(pg_sys::Oid, TriggerSet)], tview_entity: &str) -> TViewResult<()> {
    // A trigger argument written as a quoted identifier is stored as its name.
    let entity_arg = quote_identifier(tview_entity);
    for &(table_oid, set) in plan {
        let (schema, relname, partitioned) = get_table_name(table_oid)?;
        let qi_table = format!(
            "{}.{}",
            quote_identifier(&schema),
            quote_identifier(&relname)
        );
        let installed = entity_triggers(tview_entity, Some(table_oid))?;

        for spec in set.specs(partitioned) {
            if installed
                .iter()
                .any(|t| t.function == spec.function && t.tgtype == spec.tgtype)
            {
                continue;
            }
            let trigger_sql = format!(
                "CREATE TRIGGER {} {} EXECUTE FUNCTION {}.{}({entity_arg})",
                quote_identifier(&trigger_name(spec.tag, tview_entity, &schema, &relname)),
                spec.clause.replace("{table}", &qi_table),
                crate::utils::ext_schema(),
                spec.function,
            );
            crate::utils::spi_run_ddl(&trigger_sql).map_err(|e| TViewError::CatalogError {
                operation: format!("Install {} trigger on {qi_table}", spec.function),
                pg_error: e,
            })?;
        }
        if partitioned && set != TriggerSet::None {
            ensure_partition_triggers(table_oid)?;
        }
    }

    Ok(())
}

/// The TVIEWs whose row trigger is on `table` itself (not a copy of a parent's).
///
/// # Errors
/// Returns an error if the catalog query fails.
pub fn row_trigger_entities(table: pg_sys::Oid) -> TViewResult<Vec<String>> {
    let query = format!(
        "SELECT DISTINCT pg_catalog.convert_from( \
                    pg_catalog.substring(t.tgargs, 1, pg_catalog.length(t.tgargs) - 1), \
                    pg_catalog.getdatabaseencoding()) \
         FROM pg_catalog.pg_trigger t \
         JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
         WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
           AND p.proname = '{ROW_HANDLER}' \
           AND t.tgparentid = 0 AND t.tgnargs = 1 AND t.tgrelid = $1",
        schema = crate::utils::ext_schema(),
    );
    Spi::connect(|client| {
        let args = [crate::utils::spi::oid(table)];
        let mut out = Vec::new();
        for row in client.select(&query, None, &args)? {
            if let Some(entity) = row.get::<String>(1)? {
                out.push(entity);
            }
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Find the TVIEWs over a partitioned table".to_string(),
        pg_error: e.to_string(),
    })
}

/// Give every partition of the tree `rel` belongs to the `PARTITION_MEMBER`
/// triggers of each TVIEW whose row trigger sits on the tree's root, and remove
/// ours from members that no longer need them (a partition detached, a root no
/// longer read). Covers `rel`'s own subtree too, so after a `DETACH` it cleans
/// the detached table. Idempotent; each trigger is created or dropped as the
/// owner of its table, which may not be the caller (a partition created by a
/// partition manager's role).
///
/// # Errors
/// Returns an error if the catalog query or a trigger change fails.
pub fn ensure_partition_triggers(rel: pg_sys::Oid) -> TViewResult<()> {
    /// `(action, entity, table, trigger function, trigger name)`.
    type Change = (String, String, pg_sys::Oid, String, Option<String>);
    let query = format!(
        "WITH ours AS ( \
             SELECT t.tgname::text AS tgname, t.tgrelid, p.proname::text AS proname, \
                    pg_catalog.convert_from( \
                        pg_catalog.substring(t.tgargs, 1, pg_catalog.length(t.tgargs) - 1), \
                        pg_catalog.getdatabaseencoding()) AS entity \
             FROM pg_catalog.pg_trigger t \
             JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
             WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
               AND p.proname IN ('{ROW_HANDLER}', '{FLUSH_HANDLER}', '{DELTA_HANDLER}', \
                                 '{TRUNCATE_HANDLER}') \
               AND t.tgparentid = 0 AND t.tgnargs = 1 \
         ), \
         tree AS ( \
             SELECT m.relid, \
                    COALESCE(pg_catalog.pg_partition_root(m.relid)::pg_catalog.oid, m.relid) AS root \
             FROM (SELECT relid::pg_catalog.oid FROM pg_catalog.pg_partition_tree( \
                       COALESCE(pg_catalog.pg_partition_root($1), $1::pg_catalog.regclass)) \
                   UNION SELECT $1) m \
         ), \
         wanted AS ( \
             SELECT DISTINCT o.entity, tr.relid, f.proname \
             FROM tree tr \
             JOIN ours o ON o.tgrelid = tr.root AND o.proname = '{ROW_HANDLER}' \
             CROSS JOIN (VALUES ('{FLUSH_HANDLER}'), ('{TRUNCATE_HANDLER}')) f(proname) \
             WHERE tr.relid <> tr.root \
         ) \
         SELECT 'create', w.entity, w.relid, w.proname, NULL::pg_catalog.text \
         FROM wanted w \
         WHERE NOT EXISTS (SELECT 1 FROM ours o WHERE o.tgrelid = w.relid \
                           AND o.entity = w.entity AND o.proname = w.proname) \
         UNION ALL \
         SELECT 'drop', o.entity, o.tgrelid, o.proname, o.tgname \
         FROM ours o JOIN tree tr ON tr.relid = o.tgrelid \
         WHERE o.proname IN ('{FLUSH_HANDLER}', '{TRUNCATE_HANDLER}') \
           AND NOT EXISTS (SELECT 1 FROM wanted w WHERE w.relid = o.tgrelid \
                           AND w.entity = o.entity AND w.proname = o.proname) \
           AND NOT EXISTS (SELECT 1 FROM ours b WHERE b.tgrelid = o.tgrelid \
                           AND b.entity = o.entity \
                           AND b.proname IN ('{ROW_HANDLER}', '{DELTA_HANDLER}'))",
        schema = crate::utils::ext_schema(),
    );
    let changes: Vec<Change> = Spi::connect(|client| {
        let args = [crate::utils::spi::regclass(rel)];
        let mut out = Vec::new();
        for row in client.select(&query, None, &args)? {
            if let (Some(action), Some(entity), Some(relid), Some(proname)) = (
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<pg_sys::Oid>(3)?,
                row.get::<String>(4)?,
            ) {
                out.push((action, entity, relid, proname, row.get::<String>(5)?));
            }
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Find the partition triggers to change".to_string(),
        pg_error: e.to_string(),
    })?;

    for (action, entity, relid, proname, tgname) in changes {
        let (schema, relname, _) = get_table_name(relid)?;
        let qi_table = format!(
            "{}.{}",
            quote_identifier(&schema),
            quote_identifier(&relname)
        );
        if let Some(trigger) = tgname.filter(|_| action == "drop") {
            drop_trigger(relid, &qi_table, &trigger)?;
            continue;
        }
        let Some(spec) = PARTITION_MEMBER.iter().find(|s| s.function == proname) else {
            continue;
        };
        let _owner = crate::owner::AsOwner::of_table(relid)?;
        let trigger_sql = format!(
            "CREATE TRIGGER {} {} EXECUTE FUNCTION {}.{}({})",
            quote_identifier(&trigger_name(spec.tag, &entity, &schema, &relname)),
            spec.clause.replace("{table}", &qi_table),
            crate::utils::ext_schema(),
            spec.function,
            quote_identifier(&entity),
        );
        crate::utils::spi_run_ddl(&trigger_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Install {} trigger on partition {qi_table}", spec.function),
            pg_error: e,
        })?;
    }
    Ok(())
}

/// Make `tview_entity`'s triggers exactly those of `plan`: install the missing
/// ones and remove the others (on tables its definition no longer reads, or of a
/// kind the table no longer needs).
///
/// # Errors
/// Returns an error if the catalog query, a trigger drop or an install fails.
pub fn sync_entity_triggers(
    plan: &[(pg_sys::Oid, TriggerSet)],
    tview_entity: &str,
) -> TViewResult<()> {
    // A partition's triggers depend on its root's, which this changes: they are
    // reconciled once the planned tables are done.
    let mut partitions = Vec::new();
    for installed in entity_triggers(tview_entity, None)? {
        let wanted = match plan.iter().find(|(oid, _)| *oid == installed.table_oid) {
            Some(&(oid, set)) => {
                let partitioned = get_table_name(oid)?.2;
                set.specs(partitioned).iter().any(|spec| {
                    spec.function == installed.function && spec.tgtype == installed.tgtype
                })
            }
            None => false,
        };
        if wanted {
            continue;
        }
        if PARTITION_MEMBER
            .iter()
            .any(|spec| spec.function == installed.function)
            && crate::delta::partition_root(installed.table_oid)? != installed.table_oid
        {
            partitions.push(installed.table_oid);
        } else {
            drop_trigger(installed.table_oid, &installed.table, &installed.trigger)?;
        }
    }
    install_triggers(plan, tview_entity)?;
    for partition in partitions {
        ensure_partition_triggers(partition)?;
    }
    Ok(())
}

/// Remove every trigger installed for `tview_entity`, wherever it is. This needs
/// no dependency walk, so it still works once the backing view is gone (a base
/// table or helper view dropped with CASCADE).
///
/// # Errors
/// Returns an error if the catalog query or a trigger drop fails.
pub fn remove_entity_triggers(tview_entity: &str) -> TViewResult<()> {
    for installed in entity_triggers(tview_entity, None)? {
        drop_trigger(installed.table_oid, &installed.table, &installed.trigger)?;
    }
    Ok(())
}

/// `DROP TRIGGER IF EXISTS trigger ON table` (`table` quoted and qualified), as
/// the table's owner: `DROP TRIGGER` needs the owner where `CREATE TRIGGER` needs
/// only the `TRIGGER` privilege, and these are `pg_tviews`' own triggers, removed
/// for a TVIEW the caller may drop.
fn drop_trigger(table_oid: pg_sys::Oid, table: &str, trigger: &str) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_table(table_oid)?;
    let drop_sql = format!(
        "DROP TRIGGER IF EXISTS {} ON {table}",
        quote_identifier(trigger)
    );
    crate::utils::spi_run_ddl(&drop_sql).map_err(|e| TViewError::CatalogError {
        operation: format!("Drop trigger {trigger} from {table}"),
        pg_error: e,
    })
}

/// Returns `(schema_name, table_name)` for the given OID, both unquoted.
/// Schema, name and whether `oid` is a partitioned table.
fn get_table_name(oid: pg_sys::Oid) -> TViewResult<(String, String, bool)> {
    let names = Spi::connect(|client| {
        let args = [crate::utils::spi::oid(oid)];
        client
            .select(
                "SELECT n.nspname::text, c.relname::text, c.relkind = 'p' \
                 FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE c.oid = $1",
                None,
                &args,
            )?
            .first()
            .get_three::<String, String, bool>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get table name for OID {oid:?}"),
        pg_error: e.to_string(),
    })?;
    match names {
        (Some(schema), Some(relname), partitioned) => {
            Ok((schema, relname, partitioned.unwrap_or(false)))
        }
        _ => Err(TViewError::CatalogError {
            operation: format!("Get table name for OID {oid:?}"),
            pg_error: "not found".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::trigger_name;
    use crate::utils::MAX_IDENTIFIER_BYTES;

    #[test]
    fn test_trigger_name_short_is_verbatim() {
        assert_eq!(
            trigger_name("row", "post", "public", "tb_user"),
            "trg_tview_row_post_on_public_tb_user"
        );
        assert_eq!(
            trigger_name("flush", "post", "public", "tb_user"),
            "trg_tview_flush_post_on_public_tb_user"
        );
    }

    #[test]
    fn test_trigger_name_row_and_flush_of_other_entities_differ() {
        // Entity `flush_x`'s row trigger and entity `x`'s flush trigger.
        assert_ne!(
            trigger_name("row", "flush_x", "public", "tb_t"),
            trigger_name("flush", "x", "public", "tb_t")
        );
    }

    #[test]
    fn test_trigger_name_long_prefixes_stay_distinct() {
        let common = "invoice_line_adjustment_with_a_deliberately_long_name_";
        let a = trigger_name("row", &format!("{common}a"), "app", "tb_x");
        let b = trigger_name("row", &format!("{common}b"), "app", "tb_x");
        assert_eq!(a.len(), MAX_IDENTIFIER_BYTES);
        assert_ne!(a, b);
    }

    #[test]
    fn test_trigger_name_counts_bytes() {
        let name = trigger_name("row", "note", &"é".repeat(40), "tb_note");
        assert!(name.len() <= MAX_IDENTIFIER_BYTES);
    }
}
