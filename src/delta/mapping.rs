//! A statement's changed rows mapped to TVIEW keys through each table's mapping
//! query, or written into every row they reach as a fan-out patch.

use super::Event;
use super::render::{computed_columns, delta_sql, key_sql_type, row_pairing};
use super::transition::{
    changed_values, reads_unchanged, run_with_transition_tables, texts, with_transition_tables,
};
use crate::catalog::{KeyType, TviewMeta};
use crate::concurrency::{LockTarget, LockValue, Pass};
use crate::config::UncascadedPolicy;
use crate::dependency::triggers::{NEW_TABLE, OLD_TABLE};
use crate::error::{TViewError, TViewResult};
use crate::lineage::{DELTA, KeyMapping, MappingKind};
use crate::queue::key::KeyValue;
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;

thread_local! {
    /// `(entity, relid)` → the rendered mapping query (`None`: a relation is gone).
    static MAPPINGS: crate::cache::Memo<(String, u32), Option<String>> = crate::cache::Memo::new();
    /// `(entity, relid)` → the rendered lookup of a fan-out through a table in
    /// between (`None`: a relation is gone).
    static FANOUT_VIAS: crate::cache::Memo<(String, u32), Option<String>> =
        crate::cache::Memo::new();
    /// `(entity, relid)` → the rendered guard of an `all_keys` table (`None`: a
    /// relation is gone).
    static GUARDS: crate::cache::Memo<(String, u32), Option<String>> = crate::cache::Memo::new();
}

/// The query that maps changed rows of `base_table`, read from a relation named
/// `pg_tviews_delta`, to keys of `meta`'s TVIEW (ADR 0157), with the current names
/// of what it reads. `None` when writes to the table do not map through a query
/// of their own (`propagated`, `all_keys`) or the TVIEW does not read it.
///
/// # Errors
/// Returns an error if the query cannot be rendered.
pub(crate) fn mapping_query(
    meta: &TviewMeta,
    base_table: pg_sys::Oid,
) -> TViewResult<Option<String>> {
    let Some(mapping) = meta.key_mapping(base_table, None) else {
        return Ok(None);
    };
    match mapping.kind {
        MappingKind::Local => Ok(mapping.column.as_deref().map(|column| {
            format!(
                "SELECT DISTINCT {} FROM {DELTA}",
                crate::utils::ident::quoted(column)
            )
        })),
        MappingKind::Mapped | MappingKind::AllKeys if mapping.sql.is_some() => {
            rendered(&meta.entity_name, mapping)
        }
        _ => Ok(None),
    }
}

/// What a refresh of `meta`'s rows reads of `base_table`, for value locks (ADR
/// 0207): for each column of the table its mapping joins on (`None` when the table
/// is locked as a whole), the query from the TVIEW's keys (`$1`) to the values a
/// refresh locks. Empty when writes to the table map through no query of their own.
///
/// # Errors
/// Returns an error if a catalog read or a rendering fails.
pub(crate) fn read_set_queries(
    meta: &TviewMeta,
    base_table: pg_sys::Oid,
) -> TViewResult<Vec<(Option<String>, Option<String>)>> {
    let Some(mapping) = meta.key_mapping(base_table, None) else {
        return Ok(Vec::new());
    };
    let key_type = meta.key_type.clone();
    let mut rows = Vec::new();
    for set in &mapping.reads {
        let column = (set.attnum != 0)
            .then(|| {
                crate::utils::spi::one::<String>(
                    "SELECT attname::pg_catalog.text FROM pg_catalog.pg_attribute \
                     WHERE attrelid = $1 AND attnum = $2",
                    &[
                        crate::utils::spi::oid(base_table),
                        crate::utils::spi::int2(set.attnum),
                    ],
                )
            })
            .transpose()
            .map_err(|e| crate::utils::spi::error("the column's name", &e))?
            .flatten();
        let keys = format!("ANY ({})", crate::refresh::key_cast(&key_type, "$1", true));
        let query = crate::concurrency::reads::rendered(&meta.entity_name, mapping, set, &keys)?;
        rows.push((column, query));
    }
    Ok(rows)
}

/// What a delta trigger does with a statement's rows for one TVIEW.
#[derive(Clone)]
pub(super) enum Route {
    /// They map to no key.
    Nothing,
    /// An `all_keys` table under `full_refresh`: the whole TVIEW, if any row
    /// changed (and met the table's guard, when it has one).
    FullRefresh,
    /// A mapping query over the changed rows (`delta`, see [`delta_sql`]).
    Mapped { delta: String },
}

/// How the rows a statement changed in `table_oid` reach `meta`'s TVIEW, read
/// through `mapping`, with the write's links noted
/// ([`crate::concurrency::note_write`]).
fn route(
    trigger: &PgTrigger<'_>,
    meta: &TviewMeta,
    mapping: &KeyMapping,
    table_oid: Oid,
    event: Event,
) -> TViewResult<Route> {
    // A refresh reads every row of the tables the TVIEW reads.
    crate::catalog::stopgap::refuse_row_security(meta)?;
    let entity = meta.entity_name.as_str();
    let full_refresh = meta.policy_for(Oid::from(mapping.relid)) == UncascadedPolicy::FullRefresh;
    let computed = computed_columns(entity, table_oid)?;
    let virtual_read = computed.is_some();
    // The columns the TVIEW reads of the table, when its plan knows them and
    // they hold now (`crate::decide`): none known (every column read) otherwise.
    let read = crate::catalog::facts::read_set(meta, mapping.relid);
    let env = crate::catalog::facts::env(meta, mapping.relid);
    let trusted: Option<Vec<i16>> =
        crate::decide::trusted_read_set(&read, env).map(|r| r.iter().collect());
    let mapped = matches!(mapping.kind, MappingKind::Mapped | MappingKind::AllKeys);
    #[cfg(feature = "test-hooks")]
    if trusted.is_none()
        && event == Event::Update
        && mapped
        && !virtual_read
        // SAFETY: an AFTER statement trigger's TriggerData.
        && unsafe { reads_unchanged(trigger.trigger_data(), &mapping.attnums) }
    {
        crate::stats::count_refusal(meta, mapping.relid);
    }
    // An UPDATE that changed none of the columns the TVIEW reads maps to no key:
    // decided from the transition tables themselves, before any query. The
    // columns of the read set some row changed: all of them when any did.
    let changed: crate::decide::Attnums = match &trusted {
        Some(columns)
            if event == Event::Update
                && mapped
                && !virtual_read
                // SAFETY: an AFTER statement trigger's TriggerData.
                && unsafe {
                    reads_unchanged(
                        trigger.trigger_data(),
                        crate::mutant::drop_last("S11", columns),
                    )
                } =>
        {
            crate::decide::Attnums::new()
        }
        Some(columns) => columns.iter().collect(),
        None => crate::decide::Attnums::new(),
    };
    let reads_same = mapped
        && crate::decide::statement_skip(decide_event(event), &read, &changed, virtual_read, env);
    // The links are columns it reads: unchanged when those are. A virtual
    // generated column is NULL in the transition tables, so one among the links
    // counts as changed; the stored ones compare as they are. Columns its plan
    // does not list may link its rows: then every write changes a link.
    if trusted.is_some() {
        crate::concurrency::note_write(Some(mapping), event, |links| {
            (virtual_read && links.iter().any(|&attnum| is_virtual(table_oid, attnum)))
                // SAFETY: an AFTER statement trigger's TriggerData.
                || (!reads_same && !unsafe { reads_unchanged(trigger.trigger_data(), links) })
        });
    } else {
        crate::concurrency::note_link_change();
    }
    if reads_same {
        return Ok(Route::Nothing);
    }
    // The pair filter (S12): the columns a row's two images are compared on.
    let compared: Vec<i16> = crate::decide::pair_filter(decide_event(event), &read, env)
        .map(|r| r.iter().collect())
        .unwrap_or_default();
    let attnums: &[i16] = &compared;
    let delta = || {
        delta_sql(
            entity,
            table_oid,
            event,
            crate::mutant::drop_last("S12", attnums),
        )
    };
    // An `all_keys` table under `full_refresh` refreshes the whole TVIEW, but
    // a guarded table's rows that meet none of its unlinked reads' conditions
    // change only what its linked reads reach (#284).
    let whole = full_refresh
        && !crate::mutant::on("S14")
        && crate::decide::lookup_filter(
            full_refresh,
            mapping.guard.is_some(),
            mapping.guard.is_none() || guard_met(trigger, entity, mapping, &delta()?)?,
        );
    Ok(match mapping.kind {
        MappingKind::AllKeys if full_refresh && !whole => {
            if mapping.sql.is_some() {
                Route::Mapped { delta: delta()? }
            } else {
                Route::Nothing
            }
        }
        MappingKind::AllKeys if whole => Route::FullRefresh,
        // An `all_keys` table outside `full_refresh` still maps the reads of it
        // that can be traced.
        MappingKind::Mapped | MappingKind::AllKeys if mapping.sql.is_some() => {
            Route::Mapped { delta: delta()? }
        }
        _ => Route::Nothing,
    })
}

/// The decisions' name of a write's event.
const fn decide_event(event: Event) -> crate::decide::Event {
    match event {
        Event::Insert => crate::decide::Event::Insert,
        Event::Update => crate::decide::Event::Update,
        Event::Delete => crate::decide::Event::Delete,
    }
}

/// Whether a changed row (`delta`) of an `all_keys` table meets the conditions of
/// one of the reads that leave it unmapped: true without a guard, or when a
/// relation the guard reads is gone.
fn guard_met(
    trigger: &PgTrigger<'_>,
    entity: &str,
    mapping: &KeyMapping,
    delta: &str,
) -> TViewResult<bool> {
    if mapping.guard.is_none() {
        return Ok(true);
    }
    let key = (entity.to_string(), mapping.relid);
    let guard = if let Some(guard) = GUARDS.with(|m| m.get(&key)) {
        guard
    } else {
        let guard = render_watched(mapping.guard.as_deref().unwrap_or_default())?;
        GUARDS.with(|m| m.insert(key, guard.clone()));
        guard
    };
    let Some(guard) = guard else {
        return Ok(true);
    };
    Ok(!lookup(trigger, entity, &KeyType::Int, delta, &guard)?.is_empty())
}

/// Whether column `attnum` of table `relid` is a virtual generated column.
fn is_virtual(relid: Oid, attnum: i16) -> bool {
    // SAFETY: a catalog lookup by OID and attribute number.
    unsafe { pg_sys::get_attgenerated(relid, attnum).cast_unsigned() == b'v' }
}

/// A TVIEW a statement's rows reach: its entity, catalog row and route.
type Routed<'a> = (&'a str, std::rc::Rc<TviewMeta>, Route);

/// Map the rows a statement changed in `table_oid` to the keys of each TVIEW of
/// `entities` (the table's delta readers, in order) and enqueue them. Every
/// TVIEW's route comes first (it notes the write's links), then one pass takes
/// the value locks of all of them (ADR 0207, #277), then each is looked up.
///
/// # Errors
/// The entity whose mapping failed, with the error.
pub(super) fn map_statement<'a>(
    trigger: &PgTrigger<'_>,
    entities: &[&'a str],
    table_oid: Oid,
    event: Event,
) -> Result<(), (&'a str, TViewError)> {
    let mut routed: Vec<Routed<'a>> = Vec::with_capacity(entities.len());
    for &entity in entities {
        let meta = TviewMeta::load_reaching(entity, table_oid)
            .and_then(|meta| {
                meta.ok_or_else(|| TViewError::TviewNotFound {
                    name: entity.to_string(),
                })
            })
            .map_err(|e| (entity, e))?;
        let Some(mapping) = meta.key_mapping(table_oid, None) else {
            refresh_all(entity, "its mapping of a written table is unknown");
            continue;
        };
        let route = route(trigger, &meta, mapping, table_oid, event).map_err(|e| (entity, e))?;
        routed.push((entity, meta, route));
    }
    // Refreshes of the TVIEW rows these rows feed meet this write on the
    // values (ADR 0207): locked before any query finds rows by them.
    capture_pass(trigger, &routed, table_oid)?;
    for (entity, meta, route) in &routed {
        apply(trigger, entity, meta, route, table_oid, event).map_err(|e| (*entity, e))?;
    }
    Ok(())
}

/// Enqueue the keys of `meta`'s TVIEW the statement's rows reach by `route`, its
/// value locks taken.
fn apply(
    trigger: &PgTrigger<'_>,
    entity: &str,
    meta: &TviewMeta,
    route: &Route,
    table_oid: Oid,
    event: Event,
) -> TViewResult<()> {
    let Some(mapping) = meta.key_mapping(table_oid, None) else {
        refresh_all(entity, "its mapping of a written table is unknown");
        return Ok(());
    };
    match route {
        Route::Nothing => {}
        Route::FullRefresh => {
            let table = if event == Event::Delete {
                OLD_TABLE
            } else {
                NEW_TABLE
            };
            let changed = run_with_transition_tables(
                trigger,
                entity,
                &KeyType::Int,
                &format!("SELECT 1::pg_catalog.int8 FROM {table} LIMIT 1"),
            )?;
            if !changed.is_empty() {
                crate::concurrency::enqueue_refresh_all(entity);
            }
        }
        Route::Mapped { delta } => {
            // A fan-out copies the write for the owner's read: only where that
            // read returns the same.
            if mapping.kind == MappingKind::Mapped
                && event == Event::Update
                && fan_out(trigger, meta, table_oid, mapping)?
            {
                return Ok(());
            }
            let Some(keys_sql) = rendered(entity, mapping)? else {
                refresh_all(entity, "a relation its mapping reads is gone");
                return Ok(());
            };
            // After the writer's locks, in a new snapshot: a row a concurrent
            // transaction linked to these values is found. Rows the first
            // lookup missed have their read set locked by the flush, as every
            // row it computes.
            let keys = lookup(trigger, entity, &meta.key_type, delta, &keys_sql)?;
            if !keys.is_empty() {
                crate::queue::enqueue_refresh_bulk(entity, keys);
            }
        }
    }
    Ok(())
}

/// The keys of `entity`'s rows that the changed rows (`delta`) reach through
/// the mapping query `keys_sql`, read in a new snapshot as the TVIEW's owner.
fn lookup(
    trigger: &PgTrigger<'_>,
    entity: &str,
    key_type: &KeyType,
    delta: &str,
    keys_sql: &str,
) -> TViewResult<Vec<KeyValue>> {
    run_with_transition_tables(
        trigger,
        entity,
        key_type,
        &format!(
            "WITH {DELTA} AS ({delta}) \
             SELECT DISTINCT k::{} FROM ({keys_sql}) s(k) WHERE k IS NOT NULL",
            key_sql_type(key_type)
        ),
    )
}

/// One pass of the value locks of the TVIEWs `routed` maps through a query, for
/// the statement's changed rows: each one's writer locks and, once the
/// transaction changed a link, the read set of the rows its lookup finds now,
/// unlocked. Taken in one sorted pass, so that two statements writing the same
/// values never wait on each other in crossed orders (#277).
///
/// # Errors
/// The entity whose locks could not be read, with the error.
fn capture_pass<'a>(
    trigger: &PgTrigger<'_>,
    routed: &[Routed<'a>],
    table_oid: Oid,
) -> Result<(), (&'a str, TViewError)> {
    let mapped: Vec<(&'a str, &TviewMeta, &str)> = routed
        .iter()
        .filter_map(|(entity, meta, route)| match route {
            Route::Mapped { delta } => Some((*entity, &**meta, delta.as_str())),
            _ => None,
        })
        .collect();
    if mapped.is_empty() {
        return Ok(());
    }
    let links = crate::concurrency::links_changed();
    let mut pass = Pass::default();
    for (entity, meta, delta) in mapped {
        let Some(mapping) = meta.key_mapping(table_oid, None) else {
            continue;
        };
        let mut locks = || -> TViewResult<()> {
            crate::concurrency::changed_rows(mapping, &mut pass, |column| {
                with_transition_tables(trigger, entity, |frame, client, table| {
                    texts(
                        frame,
                        client,
                        Some(table),
                        &changed_values(delta, column),
                        &[],
                    )
                })
            })?;
            if !links {
                return Ok(());
            }
            let Some(keys_sql) = rendered(entity, mapping)? else {
                return Ok(());
            };
            let keys = lookup(trigger, entity, &meta.key_type, delta, &keys_sql)?;
            super::capture::add_read_set(meta, &keys, &mut pass)
        };
        locks().map_err(|e| (entity, e))?;
    }
    crate::concurrency::take(&pass);
    Ok(())
}

/// Write an UPDATE into every TVIEW row it reaches instead of recomputing them,
/// when the mapping has a fan-out patch and [`crate::decide::fanout_patch`]
/// allows it for every updated row: same key, and only columns the patch copies
/// into `data`, exactly, changed among those the TVIEW reads. Whether a column
/// changed is the binary comparison of its two images, in SQL. Returns false
/// (map the rows instead) otherwise.
fn fan_out(
    trigger: &PgTrigger<'_>,
    meta: &TviewMeta,
    table_oid: Oid,
    mapping: &KeyMapping,
) -> TViewResult<bool> {
    let entity = meta.entity_name.as_str();
    let (Some(fanout), Some(key_col)) = (&mapping.fanout, &mapping.key_col) else {
        return Ok(false);
    };
    let read = crate::catalog::facts::read_set(meta, mapping.relid);
    let env = crate::catalog::facts::patch_env(meta, mapping.relid);
    let Some(trusted) = crate::decide::trusted_read_set(&read, env) else {
        #[cfg(feature = "test-hooks")]
        crate::stats::count_refusal(meta, mapping.relid);
        return Ok(false);
    };
    if !env.direct_patch_enabled {
        return Ok(false);
    }
    let read_attnums: Vec<i16> = trusted.iter().collect();
    let Some((same_row, columns)) = row_pairing(table_oid, &read_attnums)? else {
        return Ok(false);
    };
    // The read columns `data` copies, each with whether the copy is exact.
    let copied: Vec<crate::decide::Copied> = read_attnums
        .iter()
        .zip(&columns)
        .filter(|(_, name)| fanout.fields.iter().any(|(c, _)| c == *name))
        .map(|(&attnum, _)| crate::decide::Copied {
            attnum,
            exact: crate::catalog::facts::copy_is_exact(mapping.relid, attnum),
        })
        .collect();
    // Both images of each updated row, and which read columns differ between
    // them byte for byte (D10). An image without its pair is a row whose
    // primary key changed: the children of its old key no longer join it,
    // which no patch of its new key writes (#288), so the rows are mapped
    // instead.
    let compared = crate::mutant::drop_last("S16", &columns);
    let differs = if compared.is_empty() {
        "pg_catalog.array_fill(false, ARRAY[0])".to_string()
    } else {
        format!(
            "ARRAY[{}]",
            compared
                .iter()
                .map(|c| {
                    let c = crate::utils::ident::quoted(c);
                    crate::refresh::rows_differ(&[format!("o.{c}")], &[format!("n.{c}")])
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let sql = format!(
        "SELECT pg_catalog.to_jsonb(o), pg_catalog.to_jsonb(n), {differs} \
         FROM {OLD_TABLE} o FULL JOIN {NEW_TABLE} n ON {same_row}"
    );
    let pairs = with_transition_tables(trigger, entity, |frame, client, table| {
        let mut pairs = Vec::new();
        for row in crate::utils::spi::kept_update_over(frame, client, table, &sql, &[])? {
            match (
                row.get::<pgrx::JsonB>(1)?,
                row.get::<pgrx::JsonB>(2)?,
                row.get::<Vec<bool>>(3)?,
            ) {
                (Some(old), Some(new), Some(differs)) => pairs.push((old.0, new.0, differs)),
                _ => return Ok(None),
            }
        }
        Ok(Some(pairs))
    })?;
    let Some(pairs) = pairs else { return Ok(false) };
    let mut patches = Vec::new();
    for (old, new, differs) in &pairs {
        let changed: crate::decide::Attnums = read_attnums
            .iter()
            .zip(differs)
            .filter(|(_, d)| **d)
            .map(|(&attnum, _)| attnum)
            .collect();
        if changed.is_empty() {
            continue;
        }
        let Some(key) = new.get(key_col).and_then(serde_json::Value::as_i64) else {
            return Ok(false);
        };
        let key_changed = old.get(key_col) != new.get(key_col);
        if !crate::decide::fanout_patch(
            crate::decide::Event::Update,
            &read,
            &changed,
            key_changed,
            true,
            &copied,
            env,
        ) {
            return Ok(false);
        }
        // Each changed column at every path it is copied to.
        let mut fields = serde_json::Map::new();
        for (_, column) in read_attnums
            .iter()
            .zip(&columns)
            .filter(|(attnum, _)| changed.contains(**attnum))
        {
            for (_, path) in fanout.fields.iter().filter(|(c, _)| c == column) {
                fields.insert(
                    crate::queue::patch::fanout_path_key(path),
                    new.get(column).cloned().unwrap_or_default(),
                );
            }
        }
        patches.push((key, fields));
    }
    // Two joins away: the rows are those whose lookup column holds a value the
    // table in between pairs with the changed key (#228).
    let patches = if fanout.via.is_some() {
        let Some(reached) = reached_through(entity, mapping, &patches)? else {
            return Ok(false);
        };
        reached
    } else {
        patches
    };
    for (key, fields) in patches {
        crate::queue::patch::record_fanout(
            (entity.to_string(), fanout.lookup_col.clone(), key),
            fields,
        );
    }
    Ok(true)
}

/// A fan-out key and the fields it writes, by path.
type FanoutFields = (i64, serde_json::Map<String, serde_json::Value>);

/// The fan-out `patches` of keys of a table two joins away, by the lookup value
/// each key reaches through the table in between (`fanout.via`): a key reaching
/// none writes nothing. The lookup reads as the TVIEW's owner, as every refresh
/// read does, so the writer needs no privilege on the table in between and its
/// policies hide none of the rows. `None` when a relation the lookup reads is
/// gone, a value it reaches is no integer, or the rows in between can't be
/// locked one by one: the rows are then mapped and recomputed.
///
/// # Errors
/// Returns an error if the lookup fails.
fn reached_through(
    entity: &str,
    mapping: &KeyMapping,
    patches: &[FanoutFields],
) -> TViewResult<Option<Vec<FanoutFields>>> {
    if patches.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(sql) = rendered_via(entity, mapping)? else {
        return Ok(None);
    };
    let template = mapping.fanout.as_ref().and_then(|f| f.via.as_deref());
    let Some((target, value)) = via_lock(entity, template.unwrap_or_default())? else {
        return Ok(None);
    };
    let keys: Vec<i64> = patches.iter().map(|(key, _)| *key).collect();
    let Some(lookup) = crate::utils::spi::LookupKeys::of(keys) else {
        return Ok(Some(Vec::new()));
    };
    // The template looks its keys up as an array; up to 128 take their shape.
    let sql = match &lookup {
        crate::utils::spi::LookupKeys::Shaped(keys) => sql.replace(
            "ANY ($1::pg_catalog.int8[])",
            &crate::utils::spi::shaped_rhs(keys.len(), 1, |p| format!("{p}::pg_catalog.int8")),
        ),
        crate::utils::spi::LookupKeys::Many(_) => sql,
    };
    let sql = format!("SELECT s.k, s.v, {} FROM ({sql}) s(k, v)", value.of("s.v"));
    let args = lookup.args();
    let plan = lookup.plan();
    crate::owner::AsOwner::run(crate::owner::Owner::of_entity(entity)?, |frame| {
        let Some(found) = locked_lookup(frame, &sql, &args, plan, &target)? else {
            return Ok(None);
        };
        let fields: std::collections::HashMap<i64, &serde_json::Map<String, serde_json::Value>> =
            patches.iter().map(|(key, fields)| (*key, fields)).collect();
        let mut reached = Vec::with_capacity(found.len());
        for row in &found {
            let parse = |i: usize| row.get(i)?.as_deref()?.parse::<i64>().ok();
            let (Some(key), Some(value)) = (parse(0), parse(1)) else {
                return Ok(None);
            };
            if let Some(fields) = fields.get(&key) {
                reached.push((value, (*fields).clone()));
            }
        }
        Ok(Some(reached))
    })
}

/// The rows of a two-hop lookup (`sql`: key, lookup value, the value's lock
/// form), each row in between it reached share-locked by its lookup value
/// (ADR 0207). The rows in between hold the link the patch follows, and the
/// patched rows don't: a concurrent re-point of one locks that value
/// exclusively, so it waits for this write, or this lookup waits for it. No row
/// of the table is locked, so the owner needs no more than SELECT on it.
///
/// The lookup runs again after its locks, in a new snapshot, until it reaches
/// no row it hasn't locked: it then drops a row a re-point moved away, whether
/// it waited for the re-point or the re-point committed between the lookup and
/// the lock.
///
/// `None` past `pg_tviews.lock_escalation_threshold` rows: a lock on the whole
/// table would wait for every writer of it, among them one moving a row to
/// the changed key, which waits for this write's own lock on that key. The
/// rows are then mapped and recomputed, which needs no lock in between: a
/// recompute reads the link itself.
///
/// # Errors
/// Returns an error if the lookup fails.
fn locked_lookup(
    frame: &crate::owner::OwnerFrame,
    sql: &str,
    args: &[pgrx::datum::DatumWithOid<'_>],
    plan: crate::utils::spi::Plan,
    target: &LockTarget,
) -> TViewResult<Option<Vec<crate::utils::spi::TextRow>>> {
    let mut locked = std::collections::HashSet::new();
    loop {
        let found = crate::utils::spi::kept_rows(frame, sql, args, plan)?;
        let new: Vec<String> = found
            .iter()
            .filter_map(|row| row.get(2).cloned().flatten())
            .filter(|value| !locked.contains(value))
            .collect();
        if new.is_empty() {
            return Ok(Some(found));
        }
        if !crate::concurrency::lock_values_unescalated(
            target,
            crate::concurrency::Side::Refresh,
            &new,
        ) {
            return Ok(None);
        }
        locked.extend(new);
    }
}

/// Where a two-hop lookup locks the rows in between it reaches: their lookup
/// value (the second column the `via` template reads, see
/// `QueryGraph::hop_via`), when it is a read set of the table in between, so
/// that every write of such a row locks it exclusively (old and new images).
/// `None` otherwise, or when the column is gone.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
fn via_lock(entity: &str, template: &str) -> TViewResult<Option<(LockTarget, LockValue)>> {
    let Some(crate::lineage::Placeholder::Column(relid, attnum)) =
        crate::lineage::template_placeholders(template)
            .get(1)
            .copied()
    else {
        return Ok(None);
    };
    let Some(meta) = TviewMeta::load_by_entity(entity)? else {
        return Ok(None);
    };
    let read = meta
        .key_mapping(Oid::from(relid), None)
        .is_some_and(|m| m.reads.iter().any(|set| set.attnum == attnum));
    if !read {
        return Ok(None);
    }
    let Some(value) = crate::concurrency::lock_value(relid, attnum)? else {
        return Ok(None);
    };
    let target = LockTarget {
        relid,
        attnums: vec![attnum],
    };
    Ok(Some((target, value)))
}

/// Refresh `entity` in full because its mapping cannot run, and say once why.
pub(super) fn refresh_all(entity: &str, why: &str) {
    crate::utils::log_once(
        &format!("unmapped:{entity}"),
        &format!(
            "tv_{entity} is refreshed in full on writes because {why}; \
             SELECT tviews.pg_tviews_reregister('{entity}') re-derives it"
        ),
    );
    crate::concurrency::enqueue_refresh_all(entity);
}

/// The mapping query of `mapping`, with the current names of what it reads
/// (cached until one of them changes).
pub(super) fn rendered(entity: &str, mapping: &KeyMapping) -> TViewResult<Option<String>> {
    let key = (entity.to_string(), mapping.relid);
    if let Some(sql) = MAPPINGS.with(|m| m.get(&key)) {
        return Ok(sql);
    }
    let sql = render_watched(mapping.sql.as_deref().unwrap_or_default())?;
    MAPPINGS.with(|m| m.insert(key, sql.clone()));
    Ok(sql)
}

/// The lookup through the table in between of `mapping`'s fan-out
/// ([`crate::lineage::plan::FanoutPatch::via`]), with the current names of what
/// it reads (cached until one of them changes).
fn rendered_via(entity: &str, mapping: &KeyMapping) -> TViewResult<Option<String>> {
    let key = (entity.to_string(), mapping.relid);
    if let Some(sql) = FANOUT_VIAS.with(|m| m.get(&key)) {
        return Ok(sql);
    }
    let template = mapping.fanout.as_ref().and_then(|f| f.via.as_deref());
    let sql = render_watched(template.unwrap_or_default())?;
    FANOUT_VIAS.with(|m| m.insert(key, sql.clone()));
    Ok(sql)
}

/// `template` with the current names of what it reads; a rename of any of them
/// invalidates the caches.
fn render_watched(template: &str) -> TViewResult<Option<String>> {
    let sql = crate::lineage::render_template(template)
        .map_err(|e| crate::utils::spi::error(template, &e))?;
    let relids: Vec<Oid> = crate::lineage::template_placeholders(template)
        .into_iter()
        .map(|p| match p {
            crate::lineage::Placeholder::Relation(r)
            | crate::lineage::Placeholder::Column(r, _) => Oid::from(r),
        })
        .collect();
    crate::cache::watch(&relids);
    Ok(sql)
}
