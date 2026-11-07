use crate::error::{TViewError, TViewResult};
use crate::utils::ident;
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

pub use crate::catalog::readers::TriggerSet;

/// The triggers a table gets for a reader of `set`.
const fn specs(set: TriggerSet, partitioned: bool) -> &'static [TriggerSpec] {
    match (set, partitioned) {
        (TriggerSet::Row, _) | (TriggerSet::Delta, true) => &[ROW, FLUSH, TRUNCATE],
        (TriggerSet::Delta, false) => &[DELTA_INSERT, DELTA_UPDATE, DELTA_DELETE, FLUSH, TRUNCATE],
        (TriggerSet::TviewDelta, _) => &[DELTA_INSERT, DELTA_UPDATE, DELTA_DELETE],
        (TriggerSet::None, _) => &[],
    }
}

/// Every base table of a TVIEW with the triggers it gets.
pub type TriggerPlan = Vec<(pg_sys::Oid, TriggerSet)>;

/// The trigger plan of a TVIEW, from its lineage: each table it reads with the
/// set its writes map to. Nothing fires a trigger on a materialized view (the
/// `ProcessUtility` hook follows its REFRESH), a foreign table or a sequence.
pub fn trigger_plan(lineage: &crate::lineage::Lineage) -> TriggerPlan {
    lineage
        .tables
        .iter()
        .filter(|t| t.unwatched.is_none())
        .map(|t| {
            let kind = crate::lineage::MappingKind::of(&t.kind);
            (
                pg_sys::Oid::from(t.relid),
                TriggerSet::of(kind, t.tview.is_some()),
            )
        })
        .collect()
}

/// Name of a trigger on `schema.relname`: `trg_tview_<tag>_on_<schema>_<table>`,
/// fitted to 63 bytes. Statement triggers fire in name order: the tag right
/// after the fixed prefix puts the delta triggers before the flush.
fn trigger_name(tag: &str, schema: &str, relname: &str) -> String {
    crate::utils::fit_identifier(format!("trg_tview_{tag}_on_{schema}_{relname}"))
}

/// The functions of `pg_tviews`' triggers, as an SQL list.
fn handlers_sql() -> String {
    format!("'{ROW_HANDLER}', '{FLUSH_HANDLER}', '{DELTA_HANDLER}', '{TRUNCATE_HANDLER}'")
}

/// A `pg_tviews` trigger on a table (not a copy `PostgreSQL` made of a
/// partitioned table's row trigger).
struct InstalledTrigger {
    name: String,
    function: String,
    tgtype: i16,
    /// An older release installed one trigger per TVIEW, the entity as argument.
    per_tview: bool,
}

/// The `pg_tviews` triggers on `table` itself, in creation order. A trigger is
/// recognised by its function (`tgfoid`), not by its name, which a rename of the
/// table or its schema leaves stale.
fn installed_triggers(table: pg_sys::Oid) -> TViewResult<Vec<InstalledTrigger>> {
    let query = format!(
        "SELECT t.tgname::text, p.proname::text, t.tgtype, t.tgnargs <> 0 \
         FROM pg_catalog.pg_trigger t \
         JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
         WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
           AND p.proname IN ({handlers}) \
           AND t.tgparentid = 0 AND t.tgrelid = $1 \
         ORDER BY t.oid",
        schema = crate::utils::ext_schema(),
        handlers = handlers_sql(),
    );
    crate::utils::spi::rows(&query, &[crate::utils::spi::oid(Some(table))], |row| {
        Ok(
            match (
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<i16>(3)?,
                row.get::<bool>(4)?,
            ) {
                (Some(name), Some(function), Some(tgtype), Some(per_tview)) => {
                    Some(InstalledTrigger {
                        name,
                        function,
                        tgtype,
                        per_tview,
                    })
                }
                _ => None,
            },
        )
    })
    .map(|rows| rows.into_iter().flatten().collect())
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Find the pg_tviews triggers of table {table:?}"),
        pg_error: e.to_string(),
    })
}

/// What the health check finds wrong with `pg_tviews`' triggers, each
/// as `<trigger> on <table>`, or `<function> on <table> (<TVIEWs>)`.
#[derive(Default)]
pub struct TriggerProblems {
    /// Triggers on a table no TVIEW needs them on.
    pub orphaned: Vec<String>,
    /// Tables a TVIEW reads, or their partitions, that lack a trigger its
    /// readers need.
    pub missing: Vec<String>,
    /// Triggers an older release installed, one per TVIEW with the entity as
    /// argument: they fail every write until `pg_tviews_reregister_all()`
    /// replaces them.
    pub per_tview: Vec<String>,
    /// `pg_tviews` triggers disabled with `ALTER TABLE … DISABLE TRIGGER`,
    /// switched to `ENABLE REPLICA TRIGGER` (they then fire only on a replica),
    /// or on a table a logical replication subscription writes (its apply worker
    /// fires no statement trigger).
    pub disabled: Vec<String>,
}

/// Check `pg_tviews`' triggers against the tables each registered TVIEW reads
/// (`tviews.reads`: ordinary and partitioned tables reached from the
/// backing view through views) and the triggers its lineage gives each table
/// ([`TriggerSet`]): a table's set is the union of its readers'. Every
/// partition of a partitioned table carrying a row trigger expects the
/// `PARTITION_MEMBER` triggers; the copies `PostgreSQL` makes of the row trigger
/// are not counted.
///
/// # Errors
/// Returns an error if the catalog query fails.
pub fn trigger_problems() -> TViewResult<TriggerProblems> {
    let query = format!(
        "WITH ours AS ( \
             SELECT t.tgname, t.tgrelid, p.proname, t.tgenabled, t.tgnargs \
             FROM pg_catalog.pg_trigger t \
             JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
             WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
               AND p.proname IN ({handlers}) \
               AND t.tgparentid = 0 \
         ), \
         reads AS ( \
             {reads} \
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
             SELECT relid, proname, \
                    pg_catalog.string_agg(DISTINCT entity, ', ' ORDER BY entity) AS entities \
             FROM ( \
                 SELECT entity, relid, proname FROM planned \
                 UNION ALL \
                 SELECT p.entity, m.relid::pg_catalog.oid, f.proname \
                 FROM planned p \
                 CROSS JOIN LATERAL pg_catalog.pg_partition_tree(p.relid) m \
                 CROSS JOIN (VALUES ('{FLUSH_HANDLER}'), ('{TRUNCATE_HANDLER}')) AS f(proname) \
                 WHERE p.relkind = 'p' AND p.proname = '{ROW_HANDLER}' AND m.relid <> p.relid \
             ) e \
             GROUP BY relid, proname \
         ) \
         SELECT 'orphaned', pg_catalog.format('%I on %s', o.tgname, \
                                              o.tgrelid::pg_catalog.regclass) \
         FROM ours o \
         WHERE o.tgnargs = 0 \
           AND NOT EXISTS (SELECT 1 FROM expected e \
                           WHERE e.relid = o.tgrelid AND e.proname = o.proname) \
         UNION ALL \
         SELECT 'missing', pg_catalog.format('%s on %s (%s)', e.proname, \
                                             e.relid::pg_catalog.regclass, e.entities) \
         FROM expected e \
         WHERE NOT EXISTS (SELECT 1 FROM ours o \
                           WHERE o.tgnargs = 0 AND o.tgrelid = e.relid \
                             AND o.proname = e.proname) \
         UNION ALL \
         SELECT 'per_tview', pg_catalog.format('%I on %s', o.tgname, \
                                               o.tgrelid::pg_catalog.regclass) \
         FROM ours o WHERE o.tgnargs <> 0 \
         UNION ALL \
         SELECT 'disabled', pg_catalog.format('%I on %s%s', o.tgname, \
                                              o.tgrelid::pg_catalog.regclass, \
                                              CASE o.tgenabled WHEN 'R' THEN \
                                                ' (ENABLE REPLICA: fires only when \
                                                 session_replication_role is replica)' \
                                              WHEN 'D' THEN '' \
                                              ELSE ' (a logical replication subscription \
                                                 writes the table: the apply worker fires \
                                                 no statement trigger, its writes are not \
                                                 maintained)' END) \
         FROM ours o WHERE o.tgenabled IN ('D', 'R') \
            OR EXISTS (SELECT 1 FROM pg_catalog.pg_subscription_rel s \
                       WHERE s.srrelid = o.tgrelid) \
         ORDER BY 1, 2",
        schema = crate::utils::ext_schema(),
        handlers = handlers_sql(),
        reads = crate::catalog::registered::reads_with_kind_sql(),
    );
    crate::utils::spi::connect(|client| {
        let mut problems = TriggerProblems::default();
        for row in client.select(&query, None, &[])? {
            if let (Some(kind), Some(what)) = (row.get::<String>(1)?, row.get::<String>(2)?) {
                match kind.as_str() {
                    "orphaned" => problems.orphaned.push(what),
                    "missing" => problems.missing.push(what),
                    "disabled" => problems.disabled.push(what),
                    _ => problems.per_tview.push(what),
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

/// Refuse what `entity`'s triggers would need on the tables of `plan` that the
/// caller may not do: a table it did not read before (`read_before`) and holds
/// no `TRIGGER` privilege on (creating the first TVIEW over it creates the
/// triggers; a later one shares them, and may no more attach refreshes to the
/// table's writes; another TVIEW's table needs no trigger, but its writes
/// refresh `entity`'s just the same), or one a logical replication subscription
/// writes.
///
/// # Errors
/// [`TViewError::PermissionDenied`] or [`TViewError::DefinitionRefused`] naming
/// the table, or a catalog error.
pub fn check_installable(
    plan: &[(pg_sys::Oid, TriggerSet)],
    read_before: &[pg_sys::Oid],
    entity: &str,
) -> TViewResult<()> {
    let tviews = crate::catalog::tview_tables()?;
    for &(table_oid, set) in plan {
        // Another TVIEW's table needs no trigger of its own, but its writes
        // refresh this TVIEW: the same privilege as a base table's.
        let embedded = tviews.contains(&table_oid);
        if set == TriggerSet::None && !embedded {
            continue;
        }
        let (schema, relname, _) = get_table_name(table_oid)?;
        let qi_table = format!("{}.{}", ident::quoted(&schema), ident::quoted(&relname));
        let subscribed = crate::utils::spi::connect(|client| {
            client
                .select(
                    "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_subscription_rel s \
                                    WHERE s.srrelid = $1)",
                    None,
                    &[crate::utils::spi::oid(Some(table_oid))],
                )?
                .first()
                .get_one::<bool>()
        })
        .map_err(|e| TViewError::CatalogError {
            operation: format!("Check the triggers tv_{entity} needs on {qi_table}"),
            pg_error: e.to_string(),
        })?;
        if !crate::visibility::caller_may_trigger(table_oid) && !read_before.contains(&table_oid) {
            let how = if embedded {
                format!("this TVIEW's writes would refresh tv_{entity}")
            } else {
                format!("pg_tviews maintains tv_{entity} with triggers on the tables it reads")
            };
            return Err(TViewError::PermissionDenied {
                reason: format!(
                    "permission denied for table {qi_table}: {how}, which needs the TRIGGER \
                     privilege"
                ),
            });
        }
        if set == TriggerSet::None {
            continue;
        }
        if subscribed == Some(true) {
            return Err(TViewError::DefinitionRefused {
                reason: format!(
                    "tv_{entity} reads {qi_table}, which a logical replication subscription \
                     writes: its apply worker fires no statement trigger, so pg_tviews cannot \
                     maintain the TVIEW there. Create the TVIEW on the publisher and publish \
                     its table instead"
                ),
            });
        }
    }
    Ok(())
}

/// A table whose triggers [`sync_tables`] makes: its partition tree's root and
/// relkind.
struct Member {
    relid: pg_sys::Oid,
    root: pg_sys::Oid,
    partitioned: bool,
    /// No trigger watches it (a materialized view, a foreign table, a sequence):
    /// a plan reads it, but it gets no trigger, as at creation ([`trigger_plan`]).
    unwatched: bool,
}

/// Each table of `tables` with every member of its partition tree and of its
/// own subtree (after a `DETACH`, the detached table's).
fn members(tables: &[pg_sys::Oid]) -> TViewResult<Vec<Member>> {
    let query = "SELECT m.relid, \
                        COALESCE(pg_catalog.pg_partition_root(m.relid)::pg_catalog.oid, m.relid), \
                        c.relkind = 'p', c.relkind \
                 FROM (SELECT relid::pg_catalog.oid FROM pg_catalog.pg_partition_tree( \
                           COALESCE(pg_catalog.pg_partition_root($1), $1)) \
                       UNION SELECT relid::pg_catalog.oid FROM pg_catalog.pg_partition_tree($1) \
                       UNION SELECT $1::pg_catalog.oid) m \
                 JOIN pg_catalog.pg_class c ON c.oid = m.relid \
                 ORDER BY m.relid";
    let mut out: Vec<Member> = Vec::new();
    for &table in tables {
        let found = crate::utils::spi::rows(query, &[crate::utils::spi::regclass(table)], |row| {
            Ok(
                match (
                    row.get::<pg_sys::Oid>(1)?,
                    row.get::<pg_sys::Oid>(2)?,
                    row.get::<bool>(3)?,
                    row.get::<i8>(4)?,
                ) {
                    (Some(relid), Some(root), Some(partitioned), Some(relkind)) => Some(Member {
                        relid,
                        root,
                        partitioned,
                        unwatched: crate::lineage::Unwatched::of_relkind(relkind.cast_unsigned())
                            .is_some(),
                    }),
                    _ => None,
                },
            )
        })?;
        for member in found.into_iter().flatten() {
            if !out.iter().any(|m| m.relid == member.relid) {
                out.push(member);
            }
        }
    }
    Ok(out)
}

/// Make the `pg_tviews` triggers of each table of `tables`, and of every member
/// of its partition tree, the one set its readers need now (ADR 0136, D4): the
/// union of what each TVIEW whose plan maps the table needs there
/// ([`TriggerSet`]), and on a partition the `PARTITION_MEMBER` triggers when its
/// tree's root has a row trigger. Missing ones are created, the others dropped:
/// one no reader needs, a duplicate, one an older release installed per TVIEW.
/// The catalog is the only count; call it with every table a change of the
/// catalog may have touched (a TVIEW's old and new reads). Nothing is dropped
/// from a table a TVIEW whose plan does not decode reads, nor from any table
/// when not even the tables it reads can be told.
///
/// A trigger on a table is created as the caller (the TVIEW's creator holds the
/// `TRIGGER` privilege, [`check_installable`]); one on a partition, and every
/// drop, as the table's owner, who may not be the caller (a partition created by
/// a partition manager's role, a TVIEW dropped by its owner).
///
/// # Errors
/// Returns an error if the catalog cannot be read, or a trigger change fails.
pub fn sync_tables(tables: &[pg_sys::Oid]) -> TViewResult<()> {
    if tables.is_empty() {
        return Ok(());
    }
    let (plans, unreadable) = crate::catalog::readers::plans_now()?;
    let mut kept_whole = false;
    let mut kept = Vec::new();
    for entity in &unreadable {
        let tables = crate::catalog::readers::tables_of(entity)?;
        kept_whole |= tables.is_empty();
        kept.extend(tables);
    }
    let readers = |relid: pg_sys::Oid, partitioned: bool| {
        crate::catalog::readers::TableReaders::from_plans(&plans, relid, partitioned)
    };
    for member in members(tables)? {
        let mut wanted: Vec<&TriggerSpec> = Vec::new();
        if member.unwatched {
            let keep = kept_whole || kept.contains(&member.relid);
            sync_table(&member, &wanted, keep)?;
            continue;
        }
        for reader in &readers(member.relid, member.partitioned).readers {
            wanted.extend(specs(reader.set, member.partitioned));
        }
        if member.relid != member.root {
            // SAFETY: a syscache lookup by OID.
            let root_partitioned = unsafe { pg_sys::get_rel_relkind(member.root) }
                == pg_sys::RELKIND_PARTITIONED_TABLE.cast_signed();
            if readers(member.root, root_partitioned)
                .row()
                .next()
                .is_some()
            {
                wanted.extend(PARTITION_MEMBER);
            }
        }
        let keep = kept_whole || kept.contains(&member.relid) || kept.contains(&member.root);
        sync_table(&member, &wanted, keep)?;
    }
    Ok(())
}

/// Make the `pg_tviews` triggers on `member` those of `wanted`, dropping none
/// when `keep` (a plan that does not decode may need them).
fn sync_table(member: &Member, wanted: &[&TriggerSpec], keep: bool) -> TViewResult<()> {
    let installed = installed_triggers(member.relid)?;
    let (schema, relname, _) = get_table_name(member.relid)?;
    let qi_table = format!("{}.{}", ident::quoted(&schema), ident::quoted(&relname));
    let mut kept: Vec<(&str, i16)> = Vec::new();
    for trigger in &installed {
        let shape = (trigger.function.as_str(), trigger.tgtype);
        let needed = !trigger.per_tview
            && !kept.contains(&shape)
            && wanted
                .iter()
                .any(|spec| spec.function == shape.0 && spec.tgtype == shape.1);
        if needed {
            kept.push(shape);
        } else if !keep {
            drop_trigger(member.relid, &qi_table, &trigger.name)?;
        }
    }
    for spec in wanted {
        if kept.contains(&(spec.function, spec.tgtype)) {
            continue;
        }
        kept.push((spec.function, spec.tgtype));
        let trigger_sql = format!(
            "CREATE TRIGGER {} {} EXECUTE FUNCTION {}.{}()",
            ident::quoted(&trigger_name(spec.tag, &schema, &relname)),
            spec.clause.replace("{table}", &qi_table),
            crate::utils::ext_schema(),
            spec.function,
        );
        let create = || {
            crate::utils::spi::run_ddl(&trigger_sql).map_err(|e| TViewError::CatalogError {
                operation: format!("Install {} trigger on {qi_table}", spec.function),
                pg_error: e.to_string(),
            })
        };
        if member.relid == member.root {
            create()?;
        } else {
            crate::owner::AsOwner::run(crate::owner::Owner::of_table(member.relid)?, |_frame| {
                create()
            })?;
        }
    }
    Ok(())
}

/// After `entity`'s definition changed from reading `read_before` to `plan`:
/// refuse what its new reads need that the caller may not do
/// ([`check_installable`]), then make the triggers of every table it read or
/// reads now ([`sync_tables`]), and of every table carrying a `pg_tviews`
/// trigger when its old plan named no table.
///
/// # Errors
/// What [`check_installable`] or [`sync_tables`] returns.
pub fn sync_read_tables(
    plan: &[(pg_sys::Oid, TriggerSet)],
    read_before: &[pg_sys::Oid],
    entity: &str,
) -> TViewResult<()> {
    check_installable(plan, read_before, entity)?;
    let mut tables: Vec<pg_sys::Oid> = if read_before.is_empty() {
        tables_with_triggers()?
    } else {
        read_before.to_vec()
    };
    tables.extend(plan.iter().map(|&(table, _)| table));
    tables.sort_unstable_by_key(|t| t.to_u32());
    tables.dedup();
    sync_tables(&tables)
}

/// After `entity` was deregistered: make the triggers of the tables it read
/// (`read_before`, its plan's), or of every table carrying a `pg_tviews`
/// trigger when its plan named none ([`sync_tables`]).
///
/// # Errors
/// What [`sync_tables`] returns.
pub fn sync_after_drop(read_before: &[pg_sys::Oid]) -> TViewResult<()> {
    if read_before.is_empty() {
        return sync_tables(&tables_with_triggers()?);
    }
    sync_tables(read_before)
}

/// Every table carrying a `pg_tviews` trigger of its own.
///
/// # Errors
/// Returns an error if the catalog query fails.
fn tables_with_triggers() -> TViewResult<Vec<pg_sys::Oid>> {
    crate::utils::spi::oids(
        &format!(
            "SELECT DISTINCT t.tgrelid FROM pg_catalog.pg_trigger t \
             JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
             WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
               AND p.proname IN ({handlers}) AND t.tgparentid = 0",
            schema = crate::utils::ext_schema(),
            handlers = handlers_sql(),
        ),
        &[],
    )
}

/// `DROP TRIGGER IF EXISTS trigger ON table` (`table` quoted and qualified), as
/// the table's owner: `DROP TRIGGER` needs the owner where `CREATE TRIGGER` needs
/// only the `TRIGGER` privilege, and these are `pg_tviews`' own triggers, removed
/// for a TVIEW the caller may drop.
fn drop_trigger(table_oid: pg_sys::Oid, table: &str, trigger: &str) -> TViewResult<()> {
    crate::owner::AsOwner::run(crate::owner::Owner::of_table(table_oid)?, |_frame| {
        let drop_sql = format!(
            "DROP TRIGGER IF EXISTS {} ON {table}",
            ident::quoted(trigger)
        );
        crate::utils::spi::run_ddl(&drop_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Drop trigger {trigger} from {table}"),
            pg_error: e.to_string(),
        })
    })
}

/// Schema, name and whether `oid` is a partitioned table, the names unquoted.
fn get_table_name(oid: pg_sys::Oid) -> TViewResult<(String, String, bool)> {
    let names = crate::utils::spi::connect(|client| {
        let args = [crate::utils::spi::oid(Some(oid))];
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
            trigger_name("row", "public", "tb_user"),
            "trg_tview_row_on_public_tb_user"
        );
        assert_eq!(
            trigger_name("flush", "public", "tb_user"),
            "trg_tview_flush_on_public_tb_user"
        );
    }

    #[test]
    fn test_trigger_name_long_tags_stay_distinct() {
        let table = "invoice_line_adjustment_with_a_deliberately_long_name";
        let a = trigger_name("delta_i", "app", table);
        let b = trigger_name("delta_u", "app", table);
        assert_eq!(a.len(), MAX_IDENTIFIER_BYTES);
        assert_ne!(a, b);
        assert!(a.starts_with("trg_tview_delta_i_on_"));
    }

    #[test]
    fn test_trigger_name_counts_bytes() {
        let name = trigger_name("row", &"é".repeat(40), "tb_note");
        assert!(name.len() <= MAX_IDENTIFIER_BYTES);
    }
}
